#!/usr/bin/env python3
"""Build the evergreen film, README GIF and eight-second teaser.

Requires Pillow, NumPy and FFmpeg. No service, game, display or GPU access.
Usage: python docs/media/render_demo.py --output docs/media/demo.mp4
       python docs/media/render_demo.py --storyboard storyboard.jpg
       python docs/media/render_demo.py --gif-from docs/media/demo.mp4 --output docs/media/demo.gif
       python docs/media/render_demo.py --poster site/poster.jpg
The console scenes use docs/media/console/, made by capture_console.cjs.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image

import render_launch_film as film


# The cold open once the picture has reached every device.
POSTER_AT = 4.5
# Colours that cover too little of the film for a whole-film palette to keep:
# the console's live and focus colours, and the compute scene's mint hand-offs.
SMALL_COLOURS = ((76, 194, 122), (18, 41, 27), (106, 166, 240), film.MINT)


def kept_colours():
    """SMALL_COLOURS, the picture the cold open follows, and the colour scene's text."""
    colours = list(SMALL_COLOURS)
    for image, count in ((film.picture(300, 169), 6), *((image, 10) for image in film.hud())):
        palette = image.quantize(count, method=Image.Quantize.MEDIANCUT).getpalette()[:3*count]
        colours += [tuple(palette[i:i+3]) for i in range(0, 3*count, 3)]
    return colours


def encode_gif(source, destination):
    ffmpeg = shutil.which('ffmpeg')
    if not ffmpeg:
        raise RuntimeError('FFmpeg is required to encode the preview.')
    destination.parent.mkdir(parents=True, exist_ok=True)
    kept = kept_colours()
    with tempfile.TemporaryDirectory(prefix='rubylight-demo-') as directory:
        palette = Path(directory) / 'palette.png'
        base = [ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', '-threads', '4']
        subprocess.run(base + ['-i', str(source), '-vf',
                       f'fps=12,scale=960:540:flags=lanczos,palettegen=max_colors={128-len(kept)}:stats_mode=full',
                       '-frames:v', '1', str(palette)], check=True)
        # palettegen repeats its last colour up to the final entry, which it
        # reserves for transparency; the kept colours replace the repeats.
        entries = np.array(Image.open(palette).convert('RGBA')).reshape(-1, 4)
        first = int(np.flatnonzero((entries == entries[-2]).all(axis=1))[0])+1
        assert first+len(kept) < len(entries)-1
        entries[first:first+len(kept), :3] = kept
        Image.fromarray(entries.reshape(16, 16, 4), 'RGBA').save(palette)
        subprocess.run(base + ['-i', str(source), '-i', str(palette), '-filter_complex',
                       '[0:v]fps=12,scale=960:540:flags=lanczos[x];'
                       '[x][1:v]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle',
                       '-loop', '0', str(destination)], check=True)
    if destination.stat().st_size >= 10_000_000:
        raise RuntimeError(f'{destination} exceeds the 10 MB preview limit')
    print(f'Wrote {destination}', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--storyboard', type=Path)
    parser.add_argument('--frame', type=float)
    parser.add_argument('--gif-from', type=Path)
    parser.add_argument('--diagram', action='store_true')
    parser.add_argument('--poster', type=Path, help='write the website poster, a still of the cold open')
    args = parser.parse_args()
    if args.poster:
        args.poster.parent.mkdir(parents=True, exist_ok=True)
        film.frame(POSTER_AT).resize((1280, 720), Image.Resampling.LANCZOS).save(args.poster, quality=90)
        print(f'Wrote {args.poster}', flush=True)
    if args.storyboard:
        film.storyboard(args.storyboard)
    if args.diagram or args.frame is not None:
        if not args.output:
            parser.error('--diagram and --frame require --output.')
        args.output.parent.mkdir(parents=True, exist_ok=True)
        still = film.compute(8.) if args.diagram else film.frame(args.frame)
        still.save(args.output)
    elif args.gif_from:
        if not args.output:
            parser.error('--gif-from requires --output.')
        encode_gif(args.gif_from, args.output)
    elif args.output:
        ffmpeg = shutil.which('ffmpeg')
        if not ffmpeg:
            parser.error('FFmpeg is required.')
        scripts = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix='rubylight-film-') as directory:
            score = Path(directory) / 'score.wav'
            subprocess.run([sys.executable, str(scripts / 'demo_audio.py'),
                            '--output', str(score), '--duration', str(film.DURATION)], check=True)
            subprocess.run([sys.executable, str(scripts / 'render_launch_film.py'),
                            '--output', str(args.output), '--audio', str(score)], check=True)
            encode_gif(args.output, args.output.with_suffix('.gif'))
            teaser = Path(directory) / 'teaser.mp4'
            # The introduction and invitation keep the short preview self-contained.
            subprocess.run([ffmpeg, '-hide_banner', '-loglevel', 'error', '-y',
                            '-i', str(args.output), '-filter_complex',
                            '[0:v]split[a][b];[a]trim=0:4,setpts=PTS-STARTPTS[open];'
                            '[b]trim=56:60,setpts=PTS-STARTPTS[close];'
                            '[open][close]concat=n=2:v=1:a=0[v]',
                            '-map', '[v]', '-an', '-c:v', 'libx264', '-threads', '4',
                            '-crf', '19', str(teaser)], check=True)
            encode_gif(teaser, args.output.with_name(args.output.stem + '-teaser.gif'))
    elif not (args.storyboard or args.poster):
        parser.error('Choose --output, --storyboard or --poster.')


if __name__ == '__main__':
    main()
