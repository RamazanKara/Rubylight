#!/usr/bin/env python3
"""Build the product film, the README GIF and the website poster.

Requires Pillow, NumPy and FFmpeg. No service, game, display or GPU access.
Usage: python docs/media/render_demo.py --output docs/media/demo.mp4 --poster site/poster.jpg
       python docs/media/render_demo.py --storyboard storyboard.jpg
       python docs/media/render_demo.py --gif-from docs/media/demo.mp4 --output docs/media/demo.gif
       python docs/media/render_demo.py --poster site/poster.jpg
--output writes the film (1920x1080, about 3 MB) and demo.gif beside it (800 wide, under 6 MB).
The console scenes use docs/media/console/, made by capture_console.cjs. Numbers live in
NUMBERS at the top of render_launch_film.py.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

import numpy as np
from PIL import Image

import demo_audio
import render_launch_film as film

assert demo_audio.TRANSITIONS == [*film.STARTS, film.DURATION], 'the score and the film share one timeline'

POSTER_AT = film.POSTER_AT
GIF_FPS, GIF_WIDTH, GIF_LIMIT = 10, 800, 6_000_000
FILM_LIMIT = 3_500_000
# Colours that cover too little of the film for a whole-film palette to keep:
# the console's live and focus colours, and the ruby accents.
SMALL_COLOURS = ((76, 194, 122), (106, 166, 240), film.RUBY, film.ROSE, film.RUBY_DEEP)


def kept_colours():
    """SMALL_COLOURS, the game picture and the colour scene's text."""
    colours = list(SMALL_COLOURS)
    for image, count in ((film.picture(300, 169), 8), *((image, 8) for image in film.hud())):
        palette = image.quantize(count, method=Image.Quantize.MEDIANCUT).getpalette()[:3*count]
        colours += [tuple(palette[i:i+3]) for i in range(0, 3*count, 3)]
    return colours


def encode_gif(source, destination):
    ffmpeg = shutil.which('ffmpeg')
    if not ffmpeg:
        raise RuntimeError('FFmpeg is required to encode the preview.')
    destination.parent.mkdir(parents=True, exist_ok=True)
    kept = kept_colours()
    scale = f'fps={GIF_FPS},scale={GIF_WIDTH}:-2:flags=lanczos'
    with tempfile.TemporaryDirectory(prefix='rubylight-demo-') as directory:
        palette = Path(directory) / 'palette.png'
        base = [ffmpeg, '-hide_banner', '-loglevel', 'error', '-y', '-threads', '4']
        subprocess.run(base + ['-i', str(source), '-vf',
                       f'{scale},palettegen=max_colors={160-len(kept)}:stats_mode=full',
                       '-frames:v', '1', str(palette)], check=True)
        # palettegen repeats its last colour up to the final entry, which it
        # reserves for transparency; the kept colours replace the repeats.
        entries = np.array(Image.open(palette).convert('RGBA')).reshape(-1, 4)
        first = int(np.flatnonzero((entries == entries[-2]).all(axis=1))[0])+1
        assert first+len(kept) < len(entries)-1
        entries[first:first+len(kept), :3] = kept
        Image.fromarray(entries.reshape(16, 16, 4), 'RGBA').save(palette)
        subprocess.run(base + ['-i', str(source), '-i', str(palette), '-filter_complex',
                       f'[0:v]{scale}[x];'
                       '[x][1:v]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle',
                       '-loop', '0', str(destination)], check=True)
    size = destination.stat().st_size
    if size >= GIF_LIMIT:
        raise RuntimeError(f'{destination} is {size/1e6:.2f} MB, over the {GIF_LIMIT/1e6:.0f} MB README limit')
    print(f'Wrote {destination} ({size/1e6:.2f} MB)', flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--storyboard', type=Path)
    parser.add_argument('--frame', type=float)
    parser.add_argument('--gif-from', type=Path)
    parser.add_argument('--size-mb', type=float, default=2.9, help='film size; two-pass encoded to fit')
    parser.add_argument('--poster', type=Path, help='write the website poster, a still of the opening')
    args = parser.parse_args()
    if args.poster:
        args.poster.parent.mkdir(parents=True, exist_ok=True)
        film.frame(POSTER_AT).resize((1280, 720), Image.Resampling.LANCZOS).save(args.poster, quality=88)
        print(f'Wrote {args.poster}', flush=True)
    if args.storyboard:
        film.storyboard(args.storyboard)
    if args.frame is not None:
        if not args.output:
            parser.error('--frame requires --output.')
        args.output.parent.mkdir(parents=True, exist_ok=True)
        film.frame(args.frame).save(args.output)
    elif args.gif_from:
        if not args.output:
            parser.error('--gif-from requires --output.')
        encode_gif(args.gif_from, args.output)
    elif args.output:
        scripts = Path(__file__).resolve().parent
        with tempfile.TemporaryDirectory(prefix='rubylight-film-') as directory:
            score = Path(directory) / 'score.wav'
            subprocess.run([sys.executable, str(scripts / 'demo_audio.py'),
                            '--output', str(score), '--duration', str(film.DURATION)], check=True)
            cut = Path(directory) / 'gif.mkv'
            subprocess.run([sys.executable, str(scripts / 'render_launch_film.py'), '--size-mb', str(args.size_mb),
                            '--output', str(args.output), '--audio', str(score), '--gif-master', str(cut)],
                           check=True)
            size = args.output.stat().st_size
            if size >= FILM_LIMIT:
                raise RuntimeError(f'{args.output} is {size/1e6:.2f} MB; lower --size-mb')
            encode_gif(cut, args.output.with_suffix('.gif'))
    elif not (args.storyboard or args.poster):
        parser.error('Choose --output, --storyboard or --poster.')


if __name__ == '__main__':
    main()
