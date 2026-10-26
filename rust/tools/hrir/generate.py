"""Generate the headphone surround HRIRs from SADIE II (University of York).

usage: python -I rust/tools/hrir/generate.py --input D1_HRIR_SOFA.zip

Needs numpy and h5py. Download the input from the official record
(https://zenodo.org/records/10886409, file D1_HRIR_SOFA.zip) into an empty
directory of its own; the script only reads it, and refuses any other bytes.
Subject D1 is the Neumann KU100: the York database page and the SOFA metadata
both say so (the design's "D2" is the KEMAR).

Writes rust/core/src/audio_dsp/hrir_sadie2_ku100_48k.bin (little-endian f32,
9 directions x 2 ears x 256 taps; the order is DIRECTIONS, left ear first)
and its .json manifest. `audio_dsp::hrir` checks the .bin's SHA-256.

The steps, all at 48 kHz on the horizontal plane:
1. Pick the measured directions of DIRECTIONS (exact matches).
2. Diffuse-field equalization: invert the mean power spectrum of every
   measured direction and ear, third-octave smoothed, its shape limited to
   +-12 dB between 100 Hz and 16 kHz and held flat outside.
3. Make each filter minimum phase and delay it by its time of arrival. The
   upstream filters are approximately linear phase, with about 20 samples of
   pre-ringing; minimum phase keeps the magnitude and puts the near ear's peak
   at its arrival. The near ear's arrival aligns it with its minimum-phase
   version; the far ear follows by the interaural delay below 1.5 kHz.
4. Symmetrize mirror pairs (left ear at +a equals right ear at -a, and the
   ears of 0 degrees equal each other), in power and arrival time.
5. Trim the common onset, truncate to 256 taps with a 32-sample half-Hann
   fade, and assert that each filter keeps 99.5 % of its energy and that the
   front pair's near ear peaks within 8 samples.
"""
import argparse
import hashlib
import io
import json
import pathlib
import sys
import zipfile

import h5py
import numpy as np

INPUT_URL = 'https://zenodo.org/records/10886409/files/D1_HRIR_SOFA.zip'
INPUT_SHA256 = '366321fa78f211bacc0ec6bea96701625b196a5db54ec16748b0eab0b9705f75'
MEMBER = 'D1_HRIR_SOFA/D1_48K_24bit_256tap_FIR_SOFA.sofa'
RATE = 48_000
TAPS = 256
FADE = 32
FFT = 8192
# Azimuth anticlockwise (positive is left), as SOFA and SADIE II measure it.
DIRECTIONS = [0, 30, -30, 90, -90, 110, -110, 135, -135]
BAND = (100.0, 16_000.0)
LIMIT_DB = 12.0
MAX_PRE_PEAK = 8
MIN_ENERGY = 0.995
ITD_BELOW = 1500.0
MAX_ITD = 60


def attr(f, key):
    value = f.attrs[key]
    return value.decode() if isinstance(value, bytes) else str(value)


def third_octave(power, freqs):
    smoothed = np.empty_like(power)
    for i, f in enumerate(freqs):
        if f == 0:
            smoothed[i] = power[i]
            continue
        lo, hi = f / 2 ** (1 / 6), f * 2 ** (1 / 6)
        smoothed[i] = power[(freqs >= lo) & (freqs <= hi)].mean()
    return smoothed


def minimum_phase(magnitude):
    """A real minimum-phase impulse response of FFT taps for a one-sided magnitude."""
    floor = magnitude.max() * 1e-6
    log = np.log(np.maximum(magnitude, floor))
    cepstrum = np.fft.irfft(log, FFT)
    fold = np.zeros(FFT)
    fold[0] = cepstrum[0]
    fold[1:FFT // 2] = 2 * cepstrum[1:FFT // 2]
    fold[FFT // 2] = cepstrum[FFT // 2]
    return np.fft.irfft(np.exp(np.fft.rfft(fold)), FFT)


def arrival(h):
    """The time of arrival in samples: the lag that best aligns h with its minimum-phase version."""
    reference = minimum_phase(np.abs(np.fft.rfft(h, FFT)))[:len(h)]
    correlation = np.correlate(h, reference, mode='full')
    return float(np.argmax(correlation) - (len(reference) - 1))


def interaural_delay(left, right, freqs):
    """How many samples the right ear hears later than the left, from their cross-correlation
    below 1.5 kHz, where the time difference is the cue that counts."""
    low = freqs < ITD_BELOW
    cross = np.fft.irfft(np.fft.rfft(right, FFT) * np.conj(np.fft.rfft(left, FFT)) * low, FFT)
    lags = np.concatenate([cross[-MAX_ITD:], cross[:MAX_ITD + 1]])
    return float(np.argmax(lags) - MAX_ITD)


def arrivals(pair, freqs):
    """Both ears' times of arrival: the near ear's own, and the far ear's from the interaural delay
    (the far ear is low-passed by the head, so its own estimate runs late)."""
    itd = interaural_delay(pair[0], pair[1], freqs)
    if itd >= 0:
        left = arrival(pair[0])
        return left, left + itd
    right = arrival(pair[1])
    return right - itd, right


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--input', type=pathlib.Path, required=True)
    parser.add_argument('--out-dir', type=pathlib.Path,
                        default=pathlib.Path(__file__).resolve().parents[2] / 'core/src/audio_dsp')
    args = parser.parse_args()

    raw = args.input.read_bytes()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != INPUT_SHA256:
        sys.exit(f'{args.input}: SHA-256 {digest}, expected {INPUT_SHA256}')
    f = h5py.File(io.BytesIO(zipfile.ZipFile(io.BytesIO(raw)).read(MEMBER)), 'r')
    checks = {
        'DatabaseName': 'SADIE II',
        'ListenerShortName': 'D1',
        'SOFAConventions': 'SimpleFreeFieldHRIR',
    }
    for key, expected in checks.items():
        assert attr(f, key) == expected, (key, attr(f, key))
    assert 'KU100 subject' in attr(f, 'Comment'), attr(f, 'Comment')
    licence = attr(f, 'License')
    assert 'Apache License, Version 2.0' in licence and 'University of York' in licence, licence
    assert float(np.array(f['Data.SamplingRate'])[0]) == RATE
    assert not np.array(f['Data.Delay']).any()
    # Receiver 0 is the left ear (+y).
    assert np.array(f['ReceiverPosition'])[0, 1, 0] > 0
    positions = np.array(f['SourcePosition'], dtype=np.float64)
    irs = np.array(f['Data.IR'], dtype=np.float64)
    assert irs.shape[1:] == (2, TAPS), irs.shape

    freqs = np.fft.rfftfreq(FFT, 1 / RATE)
    spectra = np.abs(np.fft.rfft(irs, FFT, axis=-1))
    diffuse = third_octave((spectra ** 2).mean(axis=(0, 1)), freqs)
    eq_db = -10 * np.log10(diffuse)
    band = (freqs >= BAND[0]) & (freqs <= BAND[1])
    reference = eq_db[band].mean()
    shape = np.clip(eq_db - reference, -LIMIT_DB, LIMIT_DB)
    first, last = np.flatnonzero(band)[[0, -1]]
    shape[:first] = shape[first]
    shape[last + 1:] = shape[last]
    eq = 10 ** ((shape + reference) / 20)

    measured = {}
    for azimuth in DIRECTIONS:
        matches = np.flatnonzero((np.abs(positions[:, 1]) < 1e-6)
                                 & (np.abs((positions[:, 0] - azimuth + 180) % 360 - 180) < 1e-6))
        assert len(matches) == 1, (azimuth, matches)
        index = int(matches[0])
        toa = arrivals(irs[index], freqs)
        measured[azimuth] = [(spectra[index, ear] * eq, toa[ear]) for ear in range(2)]

    # Mirror pairs: the left ear at +a is the right ear at -a.
    symmetric = {}
    for azimuth in DIRECTIONS:
        ears = []
        for ear in range(2):
            mine = measured[azimuth][ear]
            mirror = measured[-azimuth][1 - ear]
            ears.append((np.sqrt((mine[0] ** 2 + mirror[0] ** 2) / 2), (mine[1] + mirror[1]) / 2))
        symmetric[azimuth] = ears

    onset = min(toa for ears in symmetric.values() for _, toa in ears)
    fade = np.ones(TAPS)
    fade[TAPS - FADE:] = np.cos(np.linspace(0, np.pi / 2, FADE + 1)[1:]) ** 2
    blob = np.zeros((len(DIRECTIONS), 2, TAPS), dtype=np.float64)
    directions = []
    for d, azimuth in enumerate(DIRECTIONS):
        entry = {'azimuth': azimuth, 'ears': []}
        for ear, (magnitude, toa) in enumerate(symmetric[azimuth]):
            delay = int(round(toa - onset))
            full = np.concatenate([np.zeros(delay), minimum_phase(magnitude)])
            h = full[:TAPS] * fade
            kept = float((h ** 2).sum() / (full ** 2).sum())
            peak = int(np.argmax(np.abs(h)))
            assert kept >= MIN_ENERGY, (azimuth, ear, kept)
            blob[d, ear] = h
            entry['ears'].append({'delay': delay, 'peak': peak, 'energy_kept': round(kept, 6)})
        directions.append(entry)
    for azimuth, near in ((30, 0), (-30, 1)):
        peak = directions[DIRECTIONS.index(azimuth)]['ears'][near]['peak']
        assert peak <= MAX_PRE_PEAK, (azimuth, peak)
    for d, azimuth in enumerate(DIRECTIONS):
        np.testing.assert_array_equal(blob[d, 0], blob[DIRECTIONS.index(-azimuth), 1])

    data = blob.astype('<f4').tobytes()
    args.out_dir.mkdir(parents=True, exist_ok=True)
    (args.out_dir / 'hrir_sadie2_ku100_48k.bin').write_bytes(data)
    manifest = {
        'source': {
            'database': 'SADIE II, University of York (Zenodo record 10886409, version 2-1)',
            'subject': 'D1, Neumann KU100',
            'url': INPUT_URL,
            'member': MEMBER,
            'sha256': INPUT_SHA256,
            'license': 'Apache-2.0, Copyright 2018, University of York',
        },
        'format': {
            'sample_rate': RATE,
            'layout': 'little-endian f32 [direction][ear: left, right][tap]',
            'taps': TAPS,
            'azimuth': 'degrees, anticlockwise (positive is left), elevation 0',
        },
        'processing': [
            'diffuse-field equalization: inverse of the mean power spectrum of all 8802 directions and both '
            f'ears, third-octave smoothed, shape limited to +-{LIMIT_DB:g} dB between {BAND[0]:g} Hz and '
            f'{BAND[1]:g} Hz and held flat outside',
            'minimum phase per filter, delayed by its time of arrival: the near ear by cross-correlation with its '
            f'minimum-phase version, the far ear by the interaural cross-correlation below {ITD_BELOW:g} Hz',
            'mirror pairs symmetrized in power and arrival time',
            f'common onset trimmed, truncated to {TAPS} taps with a {FADE}-sample half-Hann fade',
            f'asserted: each filter keeps {MIN_ENERGY:.1%} of its energy; the front pair near ear peaks within '
            f'{MAX_PRE_PEAK} samples',
        ],
        'tools': {'numpy': np.__version__, 'h5py': h5py.__version__},
        'directions': directions,
        'output_sha256': hashlib.sha256(data).hexdigest(),
    }
    with open(args.out_dir / 'hrir_sadie2_ku100_48k.json', 'w', newline='\n') as out:
        json.dump(manifest, out, indent=2)
        out.write('\n')
    print(manifest['output_sha256'], len(data))


if __name__ == '__main__':
    main()
