#!/usr/bin/env python3
"""4:2:0 against packed 4:4:4 picture quality on the AMD encoder (B3).

  encode  Runs `amf_quality` on PNG frame folders from chroma_fixtures.py (on the GPU machine).
          Writes one bitstream and one JSON line per run into the output folder.
  score   Decodes each bitstream with FFmpeg, rebuilds full-resolution YCbCr (a 4:2:0 picture is
          upsampled the way the host's chroma filter expects, a packed picture is unpacked), and
          compares with the source frames: PSNR and SSIM per plane (FFmpeg) and RGB PSNR.

The reference is the source frame converted to full-resolution YCbCr with the encoder's own
matrix and rounding (Rec.709 limited, as `--csc-mode 2`). 4:2:0 loses chroma detail before the
encoder sees it, so its chroma planes score below packed 4:4:4 even at an infinite bitrate;
the scores show how much of that shows up in text and UI at a stream's bitrate.
"""
import argparse
import json
import pathlib
import re
import statistics
import subprocess
import sys
import tempfile

import numpy as np

# Moonlight's encoderCscMode: the matrix is csc_mode >> 1 (0 is Rec.601, 1 is Rec.709) and the low bit is full range.
WEIGHTS = {0: (0.299, 0.587, 0.114), 1: (0.2126, 0.7152, 0.0722)}


def padded(width, height):
    return (width + 63) & ~63, (height + 7) & ~7


def reference_ycbcr(rgb, matrix=1, bits=8):
    """Limited-range YCbCr 4:4:4 codes (rows, columns, 3) of an 8-bit RGB frame, rounded like the host's shader."""
    kr, kg, kb = WEIGHTS[matrix]
    r, g, b = (rgb[..., i].astype(np.float64) / 255 for i in range(3))
    y = kr * r + kg * g + kb * b
    scale = 1 << (bits - 8)
    codes = np.stack([
        16 * scale + 219 * scale * y,
        128 * scale + 224 * scale * (b - y) / (2 * (1 - kb)),
        128 * scale + 224 * scale * (r - y) / (2 * (1 - kr)),
    ], axis=-1)
    return np.floor(np.clip(codes, 0, (1 << bits) - 1) + 0.5).astype(np.uint16)


def rgb_from_ycbcr(ycc, matrix=1, bits=8):
    """Inverse of reference_ycbcr (float, clipped, rounded) to 8-bit RGB."""
    kr, kg, kb = WEIGHTS[matrix]
    scale = 1 << (bits - 8)
    y = (ycc[..., 0].astype(np.float64) - 16 * scale) / (219 * scale)
    cb = (ycc[..., 1].astype(np.float64) - 128 * scale) / (224 * scale)
    cr = (ycc[..., 2].astype(np.float64) - 128 * scale) / (224 * scale)
    r = y + 2 * (1 - kr) * cr
    b = y + 2 * (1 - kb) * cb
    g = (y - kr * r - kb * b) / kg
    return np.floor(np.clip(np.stack([r, g, b], axis=-1) * 255, 0, 255) + 0.5).astype(np.uint8)


def upsample_chroma(plane, width, height):
    """Full-size chroma from a 4:2:0 plane sited at the left luma sample, vertically centred.

    Horizontally an odd column is the mean of its two neighbours; vertically a row is 3/4 of the
    nearer chroma row and 1/4 of the next one. Edges repeat.
    """
    c = plane.astype(np.float32)
    right = np.concatenate([c[:, 1:], c[:, -1:]], axis=1)
    wide = np.empty((c.shape[0], c.shape[1] * 2), np.float32)
    wide[:, 0::2] = c
    wide[:, 1::2] = (c + right) / 2
    up = np.concatenate([wide[:1], wide[:-1]], axis=0)
    down = np.concatenate([wide[1:], wide[-1:]], axis=0)
    tall = np.empty((wide.shape[0] * 2, wide.shape[1]), np.float32)
    tall[0::2] = 0.75 * wide + 0.25 * up
    tall[1::2] = 0.75 * wide + 0.25 * down
    return np.floor(tall[:height, :width] + 0.5).astype(np.uint16)


def split_packed(decoded, width, height, layered):
    """The three source planes (height, width) from a decoded packed picture, a dict of Y, U and V planes."""
    pw, ph = padded(width, height)
    luma, u, v = (np.asarray(decoded[k]) for k in 'yuv')
    out = [luma[:ph]]
    bottom = luma[ph:2 * ph]
    for index, even in enumerate((u, v)):
        plane = np.empty((ph, pw), np.int32)
        if layered:
            rows = np.arange(ph)
            plane[:, 0::2] = even[(rows % 2) * (ph // 2) + rows // 2]
        else:
            plane[:, 0::2] = even
        plane[:, 1::2] = bottom[:, index * (pw // 2):(index + 1) * (pw // 2)]
        out.append(plane)
    return out


def unpack(decoded, width, height, layered=False, maximum=255):
    """Full 4:4:4 planes (3, height, width) from the decoded 4:2:0 picture of a packed stream."""
    planes = split_packed(decoded, width, height, layered)
    if layered:
        for index in (1, 2):
            a = planes[index]
            base = np.repeat(np.repeat(a[0::2, 0::2], 2, axis=0), 2, axis=1)
            detail = np.clip(base + 2 * a - maximum - 1, 0, maximum)
            anchor = np.zeros(a.shape, bool)
            anchor[0::2, 0::2] = True
            planes[index] = np.where(anchor, a, detail)
    return np.stack([p[:height, :width] for p in planes]).astype(np.uint16)


def pack(source, layered=False, maximum=255):
    """The reference packer (a port of the Rust one): planes (3, height, width) to a dict of decoded planes."""
    _, height, width = source.shape
    pw, ph = padded(width, height)
    rows = np.minimum(np.arange(ph), height - 1)
    cols = np.minimum(np.arange(pw), width - 1)
    full = source[:, rows][:, :, cols].astype(np.int32)
    luma = np.zeros((2 * ph, pw), np.int32)
    luma[:ph] = full[0]
    out = dict(y=luma)
    for index in (1, 2):
        a = full[index].copy()
        if layered:
            base = np.repeat(np.repeat(a[0::2, 0::2], 2, axis=0), 2, axis=1)
            detail = np.minimum((a - base + maximum + 2) // 2, maximum)
            anchor = np.zeros(a.shape, bool)
            anchor[0::2, 0::2] = True
            a = np.where(anchor, a, detail)
        even = np.empty((ph, pw // 2), np.int32)
        if layered:
            order = np.arange(ph)
            even[(order % 2) * (ph // 2) + order // 2] = a[:, 0::2]
        else:
            even = a[:, 0::2]
        out['uv'[index - 1]] = even
        luma[ph:, (index - 1) * (pw // 2):index * (pw // 2)] = a[:, 1::2]
    return out


def psnr(a, b, peak):
    mse = float(np.mean((a.astype(np.float64) - b.astype(np.float64)) ** 2))
    return float('inf') if mse == 0 else 10 * np.log10(peak * peak / mse)


def read_planes(data, width, height, count, bits, chroma_420):
    """Split raw planar frames into per-frame dicts of Y, U and V arrays."""
    sample = 2 if bits > 8 else 1
    dtype = np.uint16 if bits > 8 else np.uint8
    cw, ch = (width // 2, height // 2) if chroma_420 else (width, height)
    size = (width * height + 2 * cw * ch) * sample
    if len(data) < size * count:
        raise SystemExit(f'decoder returned {len(data) // size} frames, expected {count}')
    frames = []
    for index in range(count):
        chunk = np.frombuffer(data, dtype, offset=index * size, count=size // sample)
        y = chunk[:width * height].reshape(height, width)
        u = chunk[width * height:width * height + cw * ch].reshape(ch, cw)
        v = chunk[width * height + cw * ch:].reshape(ch, cw)
        frames.append(dict(y=y, u=u, v=v))
    return frames


def decode(ffmpeg, bitstream, codec, width, height, count, bits):
    fmt = 'yuv420p10le' if bits > 8 else 'yuv420p'
    result = subprocess.run([ffmpeg, '-v', 'error', '-f', codec, '-i', str(bitstream), '-frames:v', str(count),
                             '-f', 'rawvideo', '-pix_fmt', fmt, 'pipe:1'], capture_output=True, check=True)
    if result.stderr:
        raise SystemExit(f'decoder reported errors: {result.stderr.decode(errors="replace")[:400]}')
    return read_planes(result.stdout, width, height, count, bits, True)


def picks(count, hold=30, scroll=30, every=3):
    """Frames to score: the first, the last still one, every `every`-th of the scroll, and the final one."""
    chosen = {0, min(hold, count) - 1, count - 1} | set(range(hold, min(hold + scroll, count), every))
    return sorted(i for i in chosen if 0 <= i < count)


def load_source(folder, indices):
    from PIL import Image
    return [np.asarray(Image.open(folder / f'{i:03d}.png').convert('RGB')) for i in indices]


def ffmpeg_scores(ffmpeg, reference, rebuilt, width, height, bits, workdir):
    """Per-frame PSNR and SSIM by plane from FFmpeg for two raw yuv444 files."""
    fmt = 'yuv444p10le' if bits > 8 else 'yuv444p'
    psnr_log, ssim_log = workdir / 'psnr.log', workdir / 'ssim.log'
    args = [ffmpeg, '-v', 'error']
    for path in (reference, rebuilt):
        args += ['-f', 'rawvideo', '-pix_fmt', fmt, '-video_size', f'{width}x{height}', '-framerate', '60', '-i', str(path)]
    args += ['-filter_complex',
             f'[0:v]split[a][b];[1:v]split[c][d];[a][c]psnr=stats_file={psnr_log.name};[b][d]ssim=stats_file={ssim_log.name}',
             '-f', 'null', '-']
    subprocess.run(args, check=True, cwd=workdir)
    rows = []
    for p, s in zip(psnr_log.read_text().splitlines(), ssim_log.read_text().splitlines()):
        pv = dict(re.findall(r'(psnr_[yuv]|psnr_avg):(\S+)', p))
        sv = dict(re.findall(r'\b([YUV]|All):(\S+)', s))
        cap = lambda x: min(float(x), 100.)  # noqa: E731  identical planes report inf
        rows.append(dict(psnr_y=cap(pv['psnr_y']), psnr_u=cap(pv['psnr_u']), psnr_v=cap(pv['psnr_v']),
                         ssim_y=float(sv['Y']), ssim_u=float(sv['U']), ssim_v=float(sv['V'])))
    return rows


def score_run(ffmpeg, folder, run, workdir, frame_stride=True):
    width, height, bits = run['width'], run['height'], 10 if run['ten_bit'] else 8
    count = run['frames']
    packing = run['packing']
    codec = 'hevc' if run['codec'] == 'hevc' else 'obu'
    maximum = (1 << bits) - 1
    indices = picks(count) if frame_stride else list(range(count))
    sources = load_source(folder, indices)
    pw, ph = padded(width, height)
    decoded = decode(ffmpeg, run['bitstream'], codec, *((pw, 2 * ph) if packing else (width, height)), count, bits)
    matrix = run.get('csc_mode', 2) >> 1
    ref_file, out_file = workdir / 'ref.yuv', workdir / 'out.yuv'
    rgb_psnr = []
    dtype = np.uint16 if bits > 8 else np.uint8
    with open(ref_file, 'wb') as ref, open(out_file, 'wb') as out:
        for source, frame in zip(sources, (decoded[i] for i in indices)):
            truth = reference_ycbcr(source, matrix, bits)
            if packing:
                planes = unpack(frame, width, height, packing == 2, maximum)
            else:
                planes = np.stack([frame['y'][:height, :width], upsample_chroma(frame['u'], width, height),
                                   upsample_chroma(frame['v'], width, height)]).astype(np.uint16)
            rebuilt = np.moveaxis(planes, 0, -1)
            for target, array in ((ref, np.moveaxis(truth, -1, 0)), (out, planes)):
                target.write(np.ascontiguousarray(array.astype(dtype)).tobytes())
            rgb_psnr.append(psnr(rgb_from_ycbcr(rebuilt, matrix, bits), source, 255))
    rows = ffmpeg_scores(ffmpeg, ref_file, out_file, width, height, bits, workdir)
    for row, value in zip(rows, rgb_psnr):
        row['psnr_rgb'] = min(value, 100.)
    for row, index in zip(rows, indices):
        row['frame'] = index
    return rows


def summarize(rows, hold=30, scroll=30):
    """Mean of each metric over the first picture, the scroll, the final still picture and all frames."""
    # Rows from `picks` carry their frame number; a full run does not, and then position is the number.
    rows = [dict(r, frame=r.get('frame', i)) for i, r in enumerate(rows)]
    last = rows[-1]['frame']
    phases = dict(first=[r for r in rows if r['frame'] == 0], scroll=[r for r in rows if hold <= r['frame'] < hold + scroll],
                  settled=[r for r in rows if r['frame'] == last], all=rows)
    out = {}
    for phase, chosen in phases.items():
        if not chosen:
            continue
        for key in chosen[0]:
            if key != 'frame':
                out[f'{phase}_{key}'] = statistics.fmean(r[key] for r in chosen)
    return out


def cmd_encode(args):
    ffmpeg = args.ffmpeg
    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    results = out / 'runs.jsonl'
    for folder in sorted(pathlib.Path(p) for p in args.fixtures):
        width, height = (int(v) for v in folder.name.rsplit('-', 1)[1].split('x'))
        count = len(list(folder.glob('*.png')))
        for packing in args.packing:
            for bitrate in args.bitrate:
                name = f'{folder.name}-p{packing}-{bitrate}' + ('-10' if args.ten_bit else '')
                stream = out / f'{name}.hevc'
                source = subprocess.Popen([ffmpeg, '-v', 'error', '-framerate', '60', '-i', str(folder / '%03d.png'),
                                           '-pix_fmt', 'bgra', '-f', 'rawvideo', '-'], stdout=subprocess.PIPE)
                cmd = [str(args.amf_quality), '--codec', args.codec, '--width', str(width), '--height', str(height),
                       '--fps', '60', '--bitrate', str(bitrate), '--frames', str(count), '--packing', str(packing),
                       '--csc-mode', '2', '--out', str(stream)] + (['--ten-bit', '1'] if args.ten_bit else [])
                done = subprocess.run(cmd, stdin=source.stdout, capture_output=True, text=True)
                source.stdout.close()
                source.wait()
                if done.returncode:
                    print(f'{name}: amf_quality exit {done.returncode}: {done.stderr[-600:]}', file=sys.stderr)
                    return done.returncode
                run = json.loads(done.stdout.strip().splitlines()[-1])
                run.update(name=name, fixture=folder.name, bitstream=str(stream), csc_mode=2, target_kbps=bitrate)
                with open(results, 'a') as f:
                    f.write(json.dumps(run) + '\n')
                print(f"{name}: {run['actual_bitrate_kbps']:.0f} kbps, encode {run['encode_mean_ms']:.2f} ms mean, "
                      f"p99 {run['encode_p99_ms']:.2f} ms", flush=True)
    return 0


def cmd_score(args):
    runs = [json.loads(line) for line in (args.runs / 'runs.jsonl').read_text().splitlines()]
    results = args.runs / 'scores.jsonl'
    done = set()
    if args.resume and results.exists():
        done = {json.loads(line)['name'] for line in results.read_text().splitlines()}
    else:
        results.write_text('')
    for run in runs:
        if run['name'] in done:
            continue
        run['bitstream'] = str(args.runs / pathlib.Path(run['bitstream']).name)
        folder = args.fixtures / run['fixture']
        with tempfile.TemporaryDirectory() as tmp:
            rows = score_run(args.ffmpeg, folder, run, pathlib.Path(tmp), not args.all_frames)
        line = dict(name=run['name'], fixture=run['fixture'], packing=run['packing'], ten_bit=run['ten_bit'],
                    target_kbps=run['target_kbps'], actual_kbps=run['actual_bitrate_kbps'],
                    encode_mean_ms=run['encode_mean_ms'], encode_p99_ms=run['encode_p99_ms'],
                    idr_bytes=run['idr_bytes'], **summarize(rows))
        with open(results, 'a') as f:
            f.write(json.dumps(line) + '\n')
        print(f"{run['name']}: settled Y/U/V {line['settled_psnr_y']:.1f}/{line['settled_psnr_u']:.1f}/"
              f"{line['settled_psnr_v']:.1f} dB, RGB {line['all_psnr_rgb']:.1f} dB", flush=True)
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    sub = parser.add_subparsers(dest='command', required=True)
    encode = sub.add_parser('encode')
    encode.add_argument('out', type=pathlib.Path)
    encode.add_argument('fixtures', nargs='+')
    encode.add_argument('--amf-quality', type=pathlib.Path, required=True)
    encode.add_argument('--ffmpeg', default='ffmpeg')
    encode.add_argument('--codec', default='hevc')
    encode.add_argument('--packing', type=int, nargs='+', default=[0, 1])
    encode.add_argument('--bitrate', type=int, nargs='+', default=[20000])
    encode.add_argument('--ten-bit', action='store_true')
    encode.set_defaults(run=cmd_encode)
    score = sub.add_parser('score')
    score.add_argument('runs', type=pathlib.Path)
    score.add_argument('fixtures', type=pathlib.Path)
    score.add_argument('--ffmpeg', default='ffmpeg')
    score.add_argument('--resume', action='store_true', help='skip runs already in scores.jsonl')
    score.add_argument('--all-frames', action='store_true', help='score every frame, not the picked ones')
    score.set_defaults(run=cmd_score)
    args = parser.parse_args(argv)
    return args.run(args)


if __name__ == '__main__':
    sys.exit(main())
