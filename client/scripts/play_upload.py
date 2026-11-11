#!/usr/bin/env python3
"""Upload a Nightjar release bundle to Google Play.

Run by .github/workflows/play.yml when a GitHub release is published. It opens
an edit, uploads the bundle with its R8 mapping and native symbols, sets the
release on one track with the release notes from
fastlane/metadata/android/<locale>/changelogs/<versionCode>.txt, and commits.

The service account key (JSON) comes from the PLAY_SERVICE_ACCOUNT_JSON
environment variable. Committing the edit sends it, with any other pending Play
Console changes, for review straight away (managed publishing is off). When Play
accepts only a draft release, the script retries as a draft and says so, and the
release is rolled out from Play Console.
"""

import argparse
import json
import os
import sys
from pathlib import Path

PACKAGE = 'com.butterpollo.client'
# Play's limit for one language's release notes.
NOTES_LIMIT = 500
# Fastlane folder name -> Play language code, where they differ.
LOCALES = {'de': 'de-DE'}
DRAFT_APP_ERROR = 'Only releases with status draft may be created on draft app'


def release_notes(metadata_dir, version_code):
    """Release notes for every locale that has a changelog for this versionCode."""
    notes = []
    for locale_dir in sorted(Path(metadata_dir).iterdir()):
        changelog = locale_dir / 'changelogs' / f'{version_code}.txt'
        if not changelog.is_file():
            continue
        text = changelog.read_text(encoding='utf-8').strip()
        if len(text) > NOTES_LIMIT:
            raise ValueError(f'{changelog} has {len(text)} characters; Google Play allows {NOTES_LIMIT}')
        if text:
            notes.append({'language': LOCALES.get(locale_dir.name, locale_dir.name), 'text': text})
    return notes


def track_release(version_code, name, status, notes):
    release = {'name': name, 'versionCodes': [str(version_code)], 'status': status}
    if notes:
        release['releaseNotes'] = notes
    return release


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--check', action='store_true',
                        help='only check access: open an edit, list the tracks and discard it')
    parser.add_argument('--bundle', type=Path)
    parser.add_argument('--mapping', type=Path)
    parser.add_argument('--native-symbols', type=Path)
    parser.add_argument('--metadata', type=Path, help='fastlane/metadata/android')
    parser.add_argument('--track', default='alpha', help='internal, alpha (closed testing), beta (open testing) or production')
    parser.add_argument('--status', default='completed', choices=['completed', 'draft'])
    parser.add_argument('--name', help='release name shown in Play Console, such as 4.12.9')
    args = parser.parse_args(argv)
    if not args.check and not (args.bundle and args.metadata and args.name):
        parser.error('--bundle, --metadata and --name are required unless --check is given')

    key = os.environ.get('PLAY_SERVICE_ACCOUNT_JSON', '')
    if not key.strip():
        sys.exit('PLAY_SERVICE_ACCOUNT_JSON is not set')

    # Imported here so the helpers above are testable without the Google client libraries.
    from google.oauth2 import service_account
    from googleapiclient.discovery import build
    from googleapiclient.errors import HttpError
    from googleapiclient.http import MediaFileUpload

    credentials = service_account.Credentials.from_service_account_info(
        json.loads(key), scopes=['https://www.googleapis.com/auth/androidpublisher'])
    edits = build('androidpublisher', 'v3', credentials=credentials, cache_discovery=False).edits()

    edit_id = edits.insert(packageName=PACKAGE, body={}).execute()['id']
    if args.check:
        tracks = edits.tracks().list(packageName=PACKAGE, editId=edit_id).execute().get('tracks', [])
        edits.delete(packageName=PACKAGE, editId=edit_id).execute()
        for track in tracks:
            codes = sorted({code for release in track.get('releases', []) for code in release.get('versionCodes', [])})
            print(f"{track['track']}: {', '.join(codes) or 'no releases'}")
        print(f'access to {PACKAGE} works; nothing was changed')
        return
    bundle = edits.bundles().upload(
        packageName=PACKAGE, editId=edit_id,
        media_body=MediaFileUpload(str(args.bundle), mimetype='application/octet-stream', resumable=True),
    ).execute()
    version_code = bundle['versionCode']
    print(f'uploaded {args.bundle.name} as versionCode {version_code}')

    for path, kind in ((args.mapping, 'proguard'), (args.native_symbols, 'nativeCode')):
        if path:
            edits.deobfuscationfiles().upload(
                packageName=PACKAGE, editId=edit_id, apkVersionCode=version_code, deobfuscationFileType=kind,
                media_body=MediaFileUpload(str(path), mimetype='application/octet-stream', resumable=True),
            ).execute()
            print(f'uploaded {path.name} ({kind})')

    notes = release_notes(args.metadata, version_code)
    if not notes:
        print(f'::warning::no changelogs/{version_code}.txt in {args.metadata}; the release has no notes')

    status = args.status
    while True:
        try:
            edits.tracks().update(
                packageName=PACKAGE, editId=edit_id, track=args.track,
                body={'track': args.track, 'releases': [track_release(version_code, args.name, status, notes)]},
            ).execute()
            break
        except HttpError as error:
            if status != 'draft' and DRAFT_APP_ERROR in str(error):
                print('::warning::Google Play takes only draft releases until the app is first published; '
                      'uploading as a draft, roll it out in Play Console')
                status = 'draft'
                continue
            raise

    edits.commit(packageName=PACKAGE, editId=edit_id).execute()
    print(f'{args.name} ({version_code}) is on the {args.track} track as {status}')


if __name__ == '__main__':
    main()
