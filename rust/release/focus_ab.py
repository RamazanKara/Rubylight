"""Encoder focus A/B: off against static (and centre), docs/encoder-focus-design.md section 9.

  python focus_ab.py estimate [--plan quick|m2|m3|m4|levels] [--centre]
  python focus_ab.py quick --work DIR --amf-quality EXE --ffmpeg EXE --package DIR --client EXE --gpu NAME --clips CLIP
                     [--port 48523] [--levels L] [--verify]
  python focus_ab.py quality --work DIR --amf-quality EXE --ffmpeg EXE --gpu NAME [--plan m2|m4] [--clips CLIP ...]
                     [--centre] [--levels L] [--verify]
  python focus_ab.py levels --work DIR --amf-quality EXE --ffmpeg EXE --gpu NAME
  python focus_ab.py timing --work DIR --package DIR --client EXE --gpu NAME [--plan m3|m4] [--port 48523] [--seconds 90]
                     [--levels L] [--verify]
  python focus_ab.py summarize WORK [WORK ...] [--out DIR]

quality (M2, or the M4 subset) runs ABBA quartets per codec, bitrate and fixture: A1 off, B1 static,
A2 off, B2 static at the request rescaled to A1/B1's actual bitrate, as amf_sweep.py does; --centre
adds C1 and C2 for centre at HEVC 10 Mbps. levels (M1) runs each encoder_focus_levels candidate once
beside an off run. Both encode the fixture's 180 frames with amf_quality.exe (the stream's encoder and
convert shader; host keys encoder_focus, encoder_focus_levels and encoder_focus_verify) and score them
with libvmaf (VMAF, PSNR-Y and SSIM per frame), with both branches trimmed to 180 frames on one time
base, on the full frame, each region of the fixture's regions.json, and for centre the middle half and
its periphery. Fixtures are made with chroma_fixtures.py in WORK/fixtures; game clips start 3 s in.
The 1440p HEVC 10-bit row scores every fixture converted to PQ, as amf_sweep.py's HDR fixture does.

timing (M3, or the M4 subset) runs e2e.py --record streams, 90 s each, on its own isolated host
(profile under WORK, ports PORT-5 to PORT+21, no display changes): ABBA at 1080p60 and 4K60, and one
off/on pair with slice delivery and one with intra refresh (the fork receiver's Xbox profile).

quick is the whole measurement in about ten minutes, refused when its estimate passes QUICK_BUDGET:
quartets at HEVC 10 Mbps on editor, mixed and one game clip, AV1 10 Mbps and H.264 20 Mbps on mixed,
then one 45 s off/on stream pair at 1080p60 HEVC 20 Mbps, with the host's default levels. Its verdict
says it is a quick pass.

Before every arm the installed host must be idle (/serverinfo without a session). Any sign of an AMD
encoder hang stops the run with exit code 3: investigate, never retry. Any other failure, including an
encoder exit code, an arm whose focus did not run as asked and a skipped stream, stops it with exit
code 1. A work folder is used once; summarize (focus_ab_result.py) reads one or more of them.
"""
import argparse, json, os, pathlib, re, subprocess, sys, time
import chroma_fixtures
from amf_sweep_result import ARMS
from focus_ab_result import CENTRE_ARMS, FRAMES, PAIR, centre_regions, host_stats, log_name, rescale, score_regions, summarize
from gpu_hang import EXIT_GPU_HANG, Guard

HERE = pathlib.Path(__file__).resolve().parent
FPS = 60
FIXTURES = chroma_fixtures.KINDS + ('mixed',)
# (codec, width, height, bitrates in kb/s, converted HDR)
QUALITY = dict(
    m2=(('hevc', 1920, 1080, (5000, 10000, 20000), False), ('av1', 1920, 1080, (10000, 20000), False),
        ('h264', 1920, 1080, (20000,), False), ('hevc', 2560, 1440, (20000,), True)),
    m4=(('hevc', 1920, 1080, (10000,), False),))
QUALITY_FIXTURES = dict(m2=FIXTURES, m4=('mixed', 'editor'))
# The quick pass: (codec, width, height, kb/s, converted HDR) with its fixtures; 'clip' is the first --clips.
QUICK = ((('hevc', 1920, 1080, 10000, False), ('editor', 'mixed', 'clip')),
         (('av1', 1920, 1080, 10000, False), ('mixed',)), (('h264', 1920, 1080, 20000, False), ('mixed',)))
QUICK_SECONDS, QUICK_BUDGET = 45, 600
CENTRE = ('hevc', 1920, 1080, 10000, False)
LEVELS = tuple(f'moving=2,still={s},ramp={r},centre=4,ctb_still={c}'
               for s in (4, 6, 8) for r in (2, 8) for c in ('1.0', '0.75'))
LEVEL_JOB, LEVEL_FIXTURES = ('hevc', 1920, 1080, 10000, False), ('mixed', 'editor')
# (comparison, WxHxFPS, kb/s, extra host config, arms, receiver environment)
TIMING = dict(
    m3=(('hevc-1080p60-20000', '1920x1080x60', 20000, (), ARMS, {}),
        ('hevc-2160p60-40000', '3840x2160x60', 40000, (), ARMS, {}),
        ('hevc-1080p60-20000-slices', '1920x1080x60', 20000, ('video_slice_delivery=2',), PAIR, {}),
        ('hevc-1080p60-20000-intra', '1920x1080x60', 20000, (), PAIR, {'BUTTERPOLLO_TEST_CLIENT_PROFILE': 'xbox'})),
    m4=(('hevc-1080p60-20000', '1920x1080x60', 20000, (), ARMS, {}),),
    quick=(('hevc-1080p60-20000', '1920x1080x60', 20000, (), PAIR, {}),))
SKIP_FIRST = 5
# Seconds, for estimate: an encode with its feed, scoring per 1080p frame area (full frame plus regions is about
# two), making one text fixture, a stream's start and stop. A CPU run of the scoring graph on 2026-10-26 took
# 5-7 s an arm and 18 s a fixture; libx265 ultrafast encoded 180 frames in 2-3 s.
ENCODE_SECONDS, SCORE_SECONDS, FIXTURE_SECONDS, STREAM_OVERHEAD = 5, 4, 20, 30
ENCODE_TIMEOUT = 180
VMAF_THREADS = 4
INSTALLED_PORTS = range(47984, 48011)
# amf_quality's own deadline for an encoder that stops returning output.
NOT_ENCODED = 'was not encoded within two seconds'


class Hang(Exception):
    """An AMD encoder hang: stop and investigate, never retry."""


class Refused(Exception):
    """The installed host is streaming, or its state is unknown."""


def idle():
    try:
        from e2e_host import installed_idle
        installed_idle()
    except AssertionError as error:
        raise Refused(f'the installed host is busy: {error}') from error
    except Exception as error:
        raise Refused(f'cannot confirm that the installed host is idle: {type(error).__name__}: {error}') from error


def check_port(port):
    if port - 5 < 1024 or port + 21 > 65535 or port + 21 >= INSTALLED_PORTS.start and port - 5 < INSTALLED_PORTS.stop:
        raise ValueError(f'port {port}: its block {port - 5}-{port + 21} must miss the installed host\'s '
                         f'{INSTALLED_PORTS.start}-{INSTALLED_PORTS.stop - 1}; the harness uses 48523')


def label(codec, hdr):
    return codec + ('10' if hdr else '')


def comparison_name(codec, width, height, kbps, hdr, fixture):
    return f'{label(codec, hdr)}-{width}x{height}-{kbps}-{fixture}'


def quality_jobs(plan, clips):
    """(job, fixture names, game clips) per codec and bitrate of a quality plan."""
    if plan == 'quick':
        return [(job, tuple(n for n in names if n != 'clip'), list(clips[:1]) if 'clip' in names else []) for job, names in QUICK]
    return [((codec, w, h, kbps, hdr), QUALITY_FIXTURES[plan], list(clips) if plan == 'm2' else [])
            for codec, w, h, rates, hdr in QUALITY.get(plan, ()) for kbps in rates]


def estimate(plan='m2', centre=False, clips=2):
    """Arms and minutes of a plan from the constants above; gpu_minutes counts encodes and streams only."""
    area = lambda w, h: w * h / (1920 * 1080)
    if plan == 'levels':
        w, h = LEVEL_JOB[1:3]
        arms = len(LEVEL_FIXTURES) * (len(LEVELS) + 1)
        jobs, seconds, gpu = [], arms * (ENCODE_SECONDS + 2 * SCORE_SECONDS) + len(LEVEL_FIXTURES) * FIXTURE_SECONDS, arms * ENCODE_SECONDS
    else:
        jobs, arms, seconds, gpu = quality_jobs(plan, ['clip'] * clips), 0, 0., 0.
    made = set()
    for (codec, w, h, kbps, hdr), names, game in jobs:
        n = len(names) + len(game)
        # Scoring covers the full frame and about one more frame of regions.
        arms, seconds, gpu = arms + 4 * n, seconds + 4 * n * (ENCODE_SECONDS + 2 * SCORE_SECONDS * area(w, h)), gpu + 4 * n * ENCODE_SECONDS
        seconds += sum(FIXTURE_SECONDS * area(w, h) for name in names if (name, w, h) not in made)
        made |= {(name, w, h) for name in names}
        if centre and plan == 'm2' and (codec, w, h, kbps, hdr) == CENTRE:
            # C1 and C2, and the centre and periphery rectangles (one more frame) on the quartet's arms.
            arms, seconds, gpu = arms + 2 * n, seconds + 2 * n * (ENCODE_SECONDS + 3 * SCORE_SECONDS) + 4 * n * SCORE_SECONDS, gpu + 2 * n * ENCODE_SECONDS
    stream_seconds = QUICK_SECONDS if plan == 'quick' else 90
    streams = sum(len(t[4]) for t in TIMING.get(plan, ()))
    seconds += streams * (stream_seconds + STREAM_OVERHEAD)
    gpu += streams * stream_seconds
    output = dict(plan=plan, encode_arms=arms, stream_arms=streams, frames_per_arm=FRAMES, minutes=round(seconds / 60, 1),
                  gpu_minutes=round(gpu / 60, 1),
                  assumptions=f'{ENCODE_SECONDS} s an encode, {SCORE_SECONDS} s to score a 1080p frame area, {FIXTURE_SECONDS} s a '
                              f'1080p fixture, {stream_seconds} s streams with {STREAM_OVERHEAD} s around each, idle host')
    if plan == 'quick':
        output['budget_minutes'] = QUICK_BUDGET / 60
    return output


def append(work, row):
    with (work / 'runs.jsonl').open('a', encoding='utf-8') as file:
        file.write(json.dumps(row) + '\n')
    print(json.dumps(dict(result=f"{row['kind']} {row['comparison']} {row['arm']}: {row['status']}")), flush=True)


def fixtures(work, names, width, height, clips=()):
    """(name, class, source, regions) per fixture: a folder of PNG frames with its regions.json, or a game clip."""
    output = []
    for name in names:
        folder = work / 'fixtures' / f'{name}-{width}x{height}'
        if not (folder / 'regions.json').exists():
            chroma_fixtures.main([str(work / 'fixtures'), '--size', f'{width}x{height}', '--kind', name, '--frames', str(FRAMES)])
        manifest = json.loads((folder / 'regions.json').read_text())
        if (manifest['width'], manifest['height'], manifest['frames']) != (width, height, FRAMES):
            raise ValueError(f'{folder}: made for {manifest["width"]}x{manifest["height"]}, {manifest["frames"]} frames')
        output.append((name, 'mixed' if name == 'mixed' else 'text', folder, manifest['regions']))
    for clip in clips:
        clip = pathlib.Path(clip)
        if not clip.is_file():
            raise FileNotFoundError(f'game clip {clip}')
        output.append((re.sub(r'[^a-z0-9]+', '-', clip.stem.lower()).strip('-'), 'game', clip, {}))
    return output


def source(path, width, height, hdr):
    """FFmpeg input options and the filter chain that makes the encoder's pictures; the reference starts from them."""
    if path.is_dir():
        inputs, chain = ['-framerate', FPS, '-i', path / '%03d.png'], ['null']
    else:
        inputs = ['-ss', '3', '-i', path]
        chain = [f'scale=w={width}:h={height}:force_original_aspect_ratio=increase:flags=lanczos', f'crop={width}:{height}']
    chain += ['format=gbrpf32le', 'zscale=tin=iec61966-2-1:t=linear:pin=709:p=709'] if hdr else ['format=bgra']
    return inputs, ','.join(chain)


def reference_chain(chain, hdr):
    return chain + (',zscale=tin=linear:pin=709:t=smpte2084:p=2020:m=2020_ncl:r=limited:npl=203,format=yuv420p10le'
                    if hdr else ',scale=out_color_matrix=bt709:out_range=tv,format=yuv420p')


def score_graph(regions, width, height, hdr, chain):
    """One pass: decoded and reference pictures trimmed to FRAMES on one time base, split, cropped to every
    rectangle and scored by libvmaf with PSNR and SSIM, each into its own JSON log. Returns the graph and outputs."""
    rects = [(region, index, rect) for region, items in regions.items() for index, rect in enumerate(items)]
    align = f'trim=end_frame={FRAMES},settb=1/{FPS},setpts=N'
    pixel = 'yuv420p10le' if hdr else 'yuv420p'
    parts = [f'[0:v]crop={width}:{height}:0:0,format={pixel},{align},split={len(rects)}' + ''.join(f'[d{k}]' for k in range(len(rects))),
             f'[1:v]{reference_chain(chain, hdr)},{align},split={len(rects)}' + ''.join(f'[r{k}]' for k in range(len(rects)))]
    for k, (region, index, (x, y, w, h)) in enumerate(rects):
        parts.append(f'[d{k}]crop={w}:{h}:{x}:{y}[dc{k}]')
        parts.append(f'[r{k}]crop={w}:{h}:{x}:{y}[rc{k}]')
        parts.append(f"[dc{k}][rc{k}]libvmaf=log_path={log_name(region, index)}:log_fmt=json:n_threads={VMAF_THREADS}"
                     f":shortest=1:repeatlast=0:feature='name=psnr|name=float_ssim'[o{k}]")
    return ';'.join(parts), [f'[o{k}]' for k in range(len(rects))]


def stop(process):
    if process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        try:
            # A process blocked in the driver exits only once the GPU reset returns its call.
            process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            print(f'process {process.pid} did not exit', file=sys.stderr)


def encode(args, directory, path, codec, width, height, hdr, request):
    """Feed the fixture through amf_quality.exe under the hang guard; its final JSON line."""
    inputs, chain = source(path, width, height, hdr)
    feed_command = [args.ffmpeg, '-v', 'error', '-nostdin', *inputs, '-vf', chain, '-frames:v', FRAMES,
                    '-pix_fmt', 'gbrpf32le' if hdr else 'bgra', '-f', 'rawvideo', '-']
    command = [args.amf_quality, '--codec', codec, '--width', width, '--height', height, '--fps', FPS, '--bitrate', request,
               '--frames', FRAMES, '--config', directory / 'sunshine.conf', '--out', directory / 'encoded.bin',
               *(['--hdr', '1'] if hdr else ['--csc-mode', '2'])]
    (directory / 'encode.command.json').write_text(json.dumps([[str(a) for a in feed_command], [str(a) for a in command]]))
    env = dict(os.environ, RUST_LOG='info', PATH=str(pathlib.Path(args.amf_quality).parent) + ';' + os.environ['PATH'])
    log = directory / 'encode.stderr.log'
    guard = Guard(log)
    flags = getattr(subprocess, 'CREATE_NO_WINDOW', 0)
    with (directory / 'feed.stderr.log').open('wb') as feed_log, (directory / 'encode.stdout.log').open('wb') as out, log.open('wb') as err:
        feed = subprocess.Popen([str(a) for a in feed_command], stdout=subprocess.PIPE, stderr=feed_log, creationflags=flags)
        try:
            encoder = subprocess.Popen([str(a) for a in command], stdin=feed.stdout, stdout=out, stderr=err, cwd=directory,
                                       env=env, creationflags=flags)
        except OSError:
            stop(feed)
            raise
        finally:
            feed.stdout.close()
        evidence, deadline = None, time.monotonic() + ENCODE_TIMEOUT
        try:
            while encoder.poll() is None and not evidence:
                evidence = guard.check()
                if time.monotonic() > deadline:
                    evidence = f'amf_quality did not finish within {ENCODE_TIMEOUT} s'
                time.sleep(.25)
        finally:
            stop(encoder)
            stop(feed)
    evidence = evidence or guard.check()
    text = log.read_text(errors='replace')
    if not evidence and NOT_ENCODED in text:
        evidence = next(line for line in text.splitlines() if NOT_ENCODED in line)
    if evidence:
        raise Hang(evidence)
    if encoder.returncode:
        # An exit code this harness does not know may be the GPU: never go on to another workload.
        raise RuntimeError(f'amf_quality exited {encoder.returncode}: {text[-2000:]}')
    if feed.returncode:
        raise RuntimeError(f'the FFmpeg feed exited {feed.returncode}: {(directory / "feed.stderr.log").read_text(errors="replace")[-2000:]}')
    lines = (directory / 'encode.stdout.log').read_text(errors='replace').strip().splitlines()
    metrics = json.loads(lines[-1]) if lines else {}
    if metrics.get('encoded_frames') != FRAMES or metrics.get('backend') != 'amf':
        raise RuntimeError(f'amf_quality did not encode {FRAMES} pictures with native AMF: {metrics}')
    return metrics, chain, inputs


def check_focus(metrics, mode):
    """amf_quality's final line (focus_mode, focus_maps, focus_uniform, focus_stale and, with verify, focus_qp_delta,
    design section 11 A2) must report focus as asked: off with no maps, or the mode with maps attached."""
    if 'focus_mode' not in metrics:
        raise RuntimeError('amf_quality reports no focus_mode: build it from a tree with the encoder focus change')
    reported = {'center': 'centre', None: 'off'}.get(metrics['focus_mode'], metrics['focus_mode'])
    maps = metrics.get('focus_maps') or 0
    if (reported, bool(maps)) != (mode, mode != 'off'):
        raise RuntimeError(f'focus was set to {mode}; amf_quality reports {metrics["focus_mode"]} with {maps} maps')


def host_config(mode, levels, verify):
    lines = [f'encoder_focus = {mode}']
    if mode != 'off':
        lines += [f'encoder_focus_levels = {levels}'] if levels else []
        lines += ['encoder_focus_verify = true'] if verify else []
    return '\n'.join(lines) + '\n'


def arm(args, kind, comparison, job, fixture, name, mode, request, levels, regions):
    codec, width, height, kbps, hdr = job
    fixture_name, fixture_class, path, _ = fixture
    directory = args.work / kind / comparison / name
    directory.mkdir(parents=True)
    row = dict(kind=kind, gpu=args.gpu, comparison=comparison, codec=label(codec, hdr), fixture=fixture_name,
               fixture_class=fixture_class, width=width, height=height, fps=FPS, hdr=hdr, target_kbps=kbps,
               requested_kbps=request, arm=name, mode=mode, levels=None if mode == 'off' else levels,
               verify=mode != 'off' and args.verify, status='failed', directory=str(directory))
    try:
        idle()
        (directory / 'sunshine.conf').write_text(host_config(mode, levels, args.verify))
        metrics, chain, inputs = encode(args, directory, path, codec, width, height, hdr, request)
        row.update(metrics)
        check_focus(metrics, mode)
        fmt = dict(h264='h264', hevc='hevc', av1='obu')[codec]
        graph, outputs = score_graph(regions, width, height, hdr, chain)
        command = [args.ffmpeg, '-hide_banner', '-nostdin', '-threads', '2', '-err_detect', 'explode', '-f', fmt,
                   '-i', 'encoded.bin', *inputs, '-filter_complex_threads', '2', '-filter_complex', graph,
                   *[a for o in outputs for a in ('-map', o)], '-f', 'null', '-']
        (directory / 'score.command.json').write_text(json.dumps([str(a) for a in command]))
        with (directory / 'score.log').open('wb') as log:
            if subprocess.run([str(a) for a in command], stdout=log, stderr=subprocess.STDOUT, cwd=directory,
                              timeout=1800, creationflags=getattr(subprocess, 'CREATE_NO_WINDOW', 0)).returncode:
                raise RuntimeError(f'scoring failed: {(directory / "score.log").read_text(errors="replace")[-2000:]}')
        row.update(scores=score_regions(directory, regions), status='ok')
    except BaseException as error:
        row['error'] = f'{type(error).__name__}: {error}'
        append(args.work, row)
        raise
    append(args.work, row)
    return row


def quartet(args, job, fixture, mode, centre=False):
    codec, width, height, kbps, hdr = job
    comparison = comparison_name(codec, width, height, kbps, hdr, fixture[0])
    regions = dict(full=[[0, 0, width, height]], **fixture[3], **(centre_regions(width, height) if centre else {}))
    run = lambda name, focus, request: arm(args, 'quality', comparison, job, fixture, name, focus, request, args.levels, regions)
    a1, b1 = run('A1', 'off', kbps), run('B1', mode, kbps)
    run('A2', 'off', kbps)
    run('B2', mode, rescale(kbps, a1, b1))
    if centre:
        c1 = run(CENTRE_ARMS[0], 'centre', kbps)
        run(CENTRE_ARMS[1], 'centre', rescale(kbps, a1, c1))


def start(args, plan):
    if (args.work / 'runs.jsonl').exists():
        raise ValueError(f'{args.work} already has runs: use a new work folder')
    args.work.mkdir(parents=True, exist_ok=True)
    (args.work / 'plan.json').write_text(json.dumps(plan, indent=2))


def require(*paths):
    for path in paths:
        if not pathlib.Path(path).is_file():
            raise FileNotFoundError(path)


def planned_quality(work, jobs):
    """The plan.json entries of every comparison, so a summary marks the ones that never ran incomplete."""
    return [dict(comparison=comparison_name(codec, w, h, kbps, hdr, name), codec=label(codec, hdr), fixture=name,
                 fixture_class=cls, target_kbps=kbps)
            for (codec, w, h, kbps, hdr), names, game in jobs for name, cls, _, _ in fixtures(work, names, w, h, game)]


def run_quality(args, jobs):
    for job, names, game in jobs:
        for fixture in fixtures(args.work, names, job[1], job[2], game):
            quartet(args, job, fixture, 'static', centre=args.centre and job == CENTRE)


def quality(args):
    if args.plan == 'm2' and len(args.clips) != 2:
        raise ValueError('M2 needs the two game clips (--clips TMNT GOLDENEYE)')
    clips = args.clips if args.plan == 'm2' else []
    require(args.amf_quality, args.ffmpeg)
    jobs = quality_jobs(args.plan, clips)
    start(args, dict(gpu=args.gpu, step=args.plan, quality=planned_quality(args.work, jobs), levels=args.levels,
                     verify=args.verify, centre=args.centre, clips=[str(c) for c in clips],
                     estimate=estimate(args.plan, args.centre, len(clips))))
    run_quality(args, jobs)


def levels(args):
    codec, w, h, kbps, hdr = LEVEL_JOB
    start(args, dict(gpu=args.gpu, step='levels', candidates=LEVELS, estimate=estimate('levels')))
    for fixture in fixtures(args.work, LEVEL_FIXTURES, w, h):
        regions = dict(full=[[0, 0, w, h]], **fixture[3])
        comparison = comparison_name(codec, w, h, kbps, hdr, fixture[0])
        arm(args, 'levels', comparison, LEVEL_JOB, fixture, 'off', 'off', kbps, None, regions)
        for index, candidate in enumerate(LEVELS):
            arm(args, 'levels', comparison, LEVEL_JOB, fixture, f'L{index}', 'static', kbps, candidate, regions)


def stream(args, comparison, mode, kbps, extra, name, receiver):
    """One e2e.py --record arm on the isolated host; its timing row."""
    idle()
    focus = 'off' if name.startswith('A') else 'static'
    configs = [f'encoder_focus={focus}', 'virtual_display_layout=extended', *extra]
    if focus != 'off':
        configs += [f'encoder_focus_levels={args.levels}'] if args.levels else []
        configs += ['encoder_focus_verify=true'] if args.verify else []
    case_name = f'{comparison}-{name}'
    command = [sys.executable, HERE / 'e2e.py', '--package', args.package, '--work', args.work, '--client', args.client,
               '--codec', 'hevc', '--mode', mode, '--seconds', args.seconds, '--bitrate', kbps, '--record',
               '--skip-first', SKIP_FIRST, '--hw-decoder', 'd3d11va', '--port', args.port, '--name', case_name,
               *[a for c in configs for a in ('--config', c)]]
    env = {k: v for k, v in os.environ.items() if k != 'BUTTERPOLLO_TEST_CLIENT_PROFILE'} | receiver
    case = args.work / f'e2e-{case_name}'
    row = dict(kind='timing', gpu=args.gpu, comparison=comparison, arm=name, mode=focus, stream_mode=mode, target_kbps=kbps,
               step=args.plan, seconds=int(args.seconds), config=configs, receiver=receiver, status='failed', directory=str(case))
    try:
        log = args.work / f'e2e-{case_name}.log'
        (args.work / f'e2e-{case_name}.command.json').write_text(json.dumps([str(a) for a in command]))
        with log.open('wb') as output:
            try:
                rc = subprocess.run([str(a) for a in command], stdout=output, stderr=subprocess.STDOUT, env=env,
                                    timeout=int(args.seconds) + 600).returncode
            except subprocess.TimeoutExpired as error:
                raise Hang(f'e2e.py did not finish within {error.timeout} s') from error
        result = json.loads((case / 'result.json').read_text()) if (case / 'result.json').exists() else {}
        if rc == EXIT_GPU_HANG:
            raise Hang(result.get('gpu_hang') or 'e2e.py exited 3')
        if rc or not result.get('passed') or result.get('skipped'):
            raise RuntimeError(f"e2e.py exited {rc}: {result.get('skipped') or result.get('failures') or log.read_text(errors='replace')[-2000:]}")
        stages = result['timing']['stages']
        row.update({f'{stage}_p99_ms': stages[stage]['p99_ms'] for stage in ('convert', 'encode', 'host_total')})
        row.update(host_stats((case / 'config' / 'logs' / 'butterpollo.log').read_text(errors='replace')))
        if any(row[k] is None for k in ('convert_p99_ms', 'encode_p99_ms', 'host_total_p99_ms', 'present_to_sent_p99_ms')):
            raise RuntimeError('the stream has no convert, encode or host_total record, or no stream timings window')
        maps = (row['focus'] or {}).get('maps', 0)
        if focus == 'off' and (maps or row['focus_effective'] not in (None, 'off')):
            raise RuntimeError(f"focus was off but the host reports {row['focus_effective']} with {maps} maps")
        if focus != 'off' and (row['focus_effective'] != focus or not maps):
            raise RuntimeError(f"focus was {focus} but the host reports {row['focus_effective']} with {maps} maps")
        if receiver.get('BUTTERPOLLO_TEST_CLIENT_PROFILE') == 'xbox' and not re.search(
                r'^CLIENT_PROFILE xbox .*\bintra_refresh=1\b', (case / 'client.log').read_text(errors='replace'), re.MULTILINE):
            raise RuntimeError('the receiver did not ask for intra refresh: build the fork client with the Xbox profile')
        row['status'] = 'ok'
    except BaseException as error:
        row['error'] = f'{type(error).__name__}: {error}'
        append(args.work, row)
        raise
    append(args.work, row)


def run_timing(args, plan):
    for comparison, mode, kbps, extra, arms, receiver in TIMING[plan]:
        for name in arms:
            stream(args, comparison, mode, kbps, extra, name, receiver)


def timing(args):
    check_port(args.port)
    require(args.package / 'butterpollo.exe', args.client)
    start(args, dict(gpu=args.gpu, step=args.plan, timing=[t[0] for t in TIMING[args.plan]], levels=args.levels,
                     verify=args.verify, seconds=args.seconds, port=args.port, estimate=estimate(args.plan)))
    run_timing(args, args.plan)


def quick(args):
    if len(args.clips) != 1:
        raise ValueError('the quick pass needs one game clip (--clips CLIP)')
    plan = estimate('quick', clips=1)
    if plan['minutes'] > QUICK_BUDGET / 60:
        raise ValueError(f"the quick pass is estimated at {plan['minutes']} minutes, over its {QUICK_BUDGET / 60:g}")
    check_port(args.port)
    require(args.amf_quality, args.ffmpeg, args.package / 'butterpollo.exe', args.client)
    args.centre, args.seconds = False, str(QUICK_SECONDS)
    jobs = quality_jobs('quick', args.clips)
    start(args, dict(gpu=args.gpu, step='quick', quality=planned_quality(args.work, jobs), timing=[t[0] for t in TIMING['quick']],
                     levels=args.levels, verify=args.verify, clips=[str(c) for c in args.clips], seconds=args.seconds,
                     port=args.port, estimate=plan))
    began = time.monotonic()
    run_quality(args, jobs)
    run_timing(args, 'quick')
    (args.work / 'elapsed.json').write_text(json.dumps(dict(seconds=round(time.monotonic() - began), estimate=plan)))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    commands = parser.add_subparsers(dest='action', required=True)
    plan = commands.add_parser('estimate')
    plan.add_argument('--plan', default='m2', choices=('quick', 'm2', 'm3', 'm4', 'levels'))
    plan.add_argument('--centre', action='store_true')
    for name, plans in (('quick', ('quick',)), ('quality', ('m2', 'm4')), ('levels', None), ('timing', ('m3', 'm4'))):
        sub = commands.add_parser(name)
        sub.add_argument('--work', required=True, type=pathlib.Path)
        sub.add_argument('--gpu', required=True, help='a label for the GPU, e.g. 7900xt or 780m')
        if plans:
            sub.add_argument('--plan', default=plans[0], choices=plans)
            sub.add_argument('--levels', help='encoder_focus_levels for the on arms; the host default otherwise')
            sub.add_argument('--verify', action='store_true', help='encoder_focus_verify on the on arms (costs encoder time)')
        else:
            sub.set_defaults(verify=False)
        if name in ('quick', 'timing'):
            sub.add_argument('--package', required=True, type=pathlib.Path)
            sub.add_argument('--client', required=True, type=pathlib.Path)
            sub.add_argument('--port', type=int, default=48523)
        if name == 'timing':
            sub.add_argument('--seconds', default='90')
        else:
            sub.add_argument('--amf-quality', required=True, type=pathlib.Path)
            sub.add_argument('--ffmpeg', required=True, type=pathlib.Path)
        if name in ('quick', 'quality'):
            sub.add_argument('--clips', nargs='*', default=[], type=pathlib.Path)
        if name == 'quality':
            sub.add_argument('--centre', action='store_true', help='C1 and C2 arms with centre at HEVC 10 Mbps')
    summary = commands.add_parser('summarize')
    summary.add_argument('dirs', nargs='+', type=pathlib.Path)
    summary.add_argument('--out', type=pathlib.Path)
    args = parser.parse_args(argv)
    if getattr(args, 'work', None):
        # Encoding and scoring run in each arm's folder, so every path they get must be absolute.
        args.work = args.work.resolve()
    if args.action == 'estimate':
        print(json.dumps(estimate(args.plan, args.centre)))
        return 0
    if args.action == 'summarize':
        summarize(args.dirs, args.out)
        return 0
    try:
        dict(quick=quick, quality=quality, levels=levels, timing=timing)[args.action](args)
    except Hang as error:
        (args.work / 'status.txt').write_text(f'STOPPED at an AMD encoder hang: {error}. Do not retry: reboot if Device '
                                              'Manager shows the Radeon with Code 31, then investigate.\n')
        print(f'focus_ab: AMD encoder hang: {error}', file=sys.stderr)
        return EXIT_GPU_HANG
    except Refused as error:
        print(f'focus_ab: refused: {error}', file=sys.stderr)
        return 1
    except Exception as error:
        if args.work.is_dir():
            (args.work / 'status.txt').write_text(f'ABORTED: {type(error).__name__}: {error}\n')
        print(f'focus_ab: {type(error).__name__}: {error}', file=sys.stderr)
        return 1
    (args.work / 'status.txt').write_text('Completed.\n')
    summarize([args.work])
    return 0


if __name__ == '__main__':
    sys.exit(main())
