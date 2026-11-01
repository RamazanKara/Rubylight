"""Ship the previous release's service while the service's version is unchanged.

butterpollo-service.exe keeps its file hash from release to release while its
version (rust/service/Cargo.toml) stays the same (service_inputs.py), so that
the reputation Windows and antivirus programs give it carries over. Its build
is reproducible, but a new MSYS2 windres or CI image could still change a byte.
So a package takes the service from the newest published release before this
one, checked against that release's SHA256SUMS, when its ProductVersion is the
current service version; a fresh build that differs from it is a warning. A
new service version, no earlier release or none with a service ships the fresh
build. rust/build.ps1 runs this for -Package:

    python rust/release/previous_service.py --exe <distribution>/butterpollo-service.exe --cache <dir> [--required]

Without --required (local builds), a previous release that cannot be fetched
keeps the fresh build with a warning; CI passes --required and fails instead.
A zip that does not match its SHA256SUMS always fails.
"""
import argparse, collections, hashlib, io, json, os, pathlib, re, sys, urllib.request, zipfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
REPOSITORY = 'RamazanKara/Rubylight'
SERVICE = 'butterpollo-service.exe'
ZIP = re.compile(r'-windows-x64\.zip$')

Choice = collections.namedtuple('Choice', 'reuse message warning')
# The previous release's service: its tag, ProductVersion, SHA-256 and bytes.
Previous = collections.namedtuple('Previous', 'tag version sha256 data')


def choose(version, fresh_sha256, previous):
    """Whether to ship the previous release's service (`previous`, a Previous,
    or None when there is no earlier release or no service in it) instead of
    the fresh build of service `version`."""
    if previous is None:
        return Choice(False, f'no earlier release has a service: shipping the fresh build of service {version}', None)
    if previous.version != version:
        return Choice(False, f'{previous.tag} has service {previous.version}, not {version}: '
                             f'shipping the fresh build', None)
    warning = None
    if previous.sha256 != fresh_sha256:
        warning = (f'the fresh build of service {version} differs from the one {previous.tag} shipped '
                   f'(fresh {fresh_sha256}, {previous.tag} {previous.sha256}); shipping {previous.tag}\'s')
    return Choice(True, f'shipping service {version} from {previous.tag} ({previous.sha256})', warning)


def version_key(tag):
    """A release tag's version as a sortable key (semver order), or None."""
    match = re.fullmatch(r'v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?', tag)
    if not match:
        return None
    numbers = tuple(int(part) for part in match.groups()[:3])
    if match[4] is None:
        return numbers + ((1,),)
    pre = tuple((0, int(part), '') if part.isdigit() else (1, 0, part) for part in match[4].split('.'))
    return numbers + ((0,) + pre,)


def previous_release(releases, current):
    """The published release with the highest version below `current` (this
    build's version), from GitHub's release list; None when there is none."""
    below = version_key(current)
    candidates = [(version_key(release['tag_name']), release) for release in releases
                  if not release.get('draft') and version_key(release['tag_name'])]
    candidates = [(key, release) for key, release in candidates if below is None or key < below]
    return max(candidates, key=lambda candidate: candidate[0])[1] if candidates else None


def checksums(text):
    """SHA256SUMS (`sha256sum *`) as {file name: hash}."""
    sums = {}
    for line in text.splitlines():
        parts = line.strip().split(maxsplit=1)
        if len(parts) == 2:
            sums[parts[1].lstrip('*')] = parts[0].lower()
    return sums


def product_version(data):
    """The ProductVersion string in an executable's version information, or None."""
    key = 'ProductVersion\0'.encode('utf-16-le')
    at = data.find(key)
    while at >= 6:
        # A String: wLength, wValueLength (in characters, with the NUL), wType,
        # the key, padding to 32 bits, the value.
        length = int.from_bytes(data[at - 4:at - 2], 'little')
        start = at + len(key)
        start += (-(start - (at - 6))) % 4
        value = data[start:start + 2 * length].decode('utf-16-le', 'replace').split('\0')[0]
        if length and re.fullmatch(r'\d+(\.\d+)*(-[0-9A-Za-z.-]+)?', value):
            return value
        at = data.find(key, at + 2)
    return None


def manifest_version(path):
    return re.search(r'(?m)^version = "([^"]+)"', path.read_text(encoding='utf-8'))[1]


def fetch_releases(repository):
    request = urllib.request.Request(f'https://api.github.com/repos/{repository}/releases?per_page=100',
                                     headers={'Accept': 'application/vnd.github+json', 'User-Agent': 'rubylight-build'})
    token = os.environ.get('GH_TOKEN') or os.environ.get('GITHUB_TOKEN')
    if token:
        request.add_header('Authorization', f'Bearer {token}')
    with urllib.request.urlopen(request, timeout=60) as response:
        return json.load(response)


def download(url, path):
    request = urllib.request.Request(url, headers={'User-Agent': 'rubylight-build'})
    part = path.with_name(path.name + '.part')
    with urllib.request.urlopen(request, timeout=300) as response, open(part, 'wb') as file:
        while chunk := response.read(1 << 20):
            file.write(chunk)
    os.replace(part, path)


class Integrity(Exception):
    """A download that does not match its release's SHA256SUMS."""


def fetch_previous(releases, current, cache):
    """The service of the release before `current` in `releases` (GitHub's
    release list), checked against that release's SHA256SUMS; None when there
    is no such release, or it has no checked zip or no service in it. Raises
    OSError (urllib's errors among them) when a download fails, Integrity when
    a zip does not match."""
    release = previous_release(releases, current)
    if release is None:
        return None
    tag = release['tag_name']
    assets = {asset['name']: asset['browser_download_url'] for asset in release.get('assets', [])}
    zips = [name for name in assets if ZIP.search(name)]
    if len(zips) != 1 or 'SHA256SUMS' not in assets:
        print(f'{tag} has no single windows-x64 zip with SHA256SUMS', file=sys.stderr)
        return None
    folder = pathlib.Path(cache) / re.sub(r'[^0-9A-Za-z.-]', '_', tag)
    folder.mkdir(parents=True, exist_ok=True)
    download(assets['SHA256SUMS'], folder / 'SHA256SUMS')
    expected = checksums((folder / 'SHA256SUMS').read_text(encoding='utf-8')).get(zips[0])
    if not expected:
        raise Integrity(f'{tag}\'s SHA256SUMS does not list {zips[0]}')
    archive = folder / zips[0]
    if not archive.is_file() or sha256(archive.read_bytes()) != expected:
        download(assets[zips[0]], archive)
    data = archive.read_bytes()
    if sha256(data) != expected:
        raise Integrity(f'{zips[0]} of {tag} has SHA-256 {sha256(data)}, its SHA256SUMS says {expected}')
    with zipfile.ZipFile(io.BytesIO(data)) as package:
        services = [name for name in package.namelist() if name.replace('\\', '/').split('/')[-1] == SERVICE]
        if not services:
            return None
        if len(services) > 1:
            raise Integrity(f'{zips[0]} of {tag} has {len(services)} copies of {SERVICE}')
        service = package.read(services[0])
    return Previous(tag, product_version(service), sha256(service), service)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def warn(message):
    prefix = '::warning::' if os.environ.get('GITHUB_ACTIONS') == 'true' else 'WARNING: '
    print(prefix + message, file=sys.stderr)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    parser.add_argument('--exe', required=True, type=pathlib.Path, help='the fresh build, replaced when reused')
    parser.add_argument('--cache', required=True, type=pathlib.Path, help='where downloads are kept')
    parser.add_argument('--service-version', default=None, help='default: rust/service/Cargo.toml')
    parser.add_argument('--release', default=None, help="this build's version; default: Cargo.toml")
    parser.add_argument('--repository', default=REPOSITORY)
    parser.add_argument('--releases', type=pathlib.Path,
                        help="GitHub's release list, read before (CI's build holds no token); default: fetched")
    parser.add_argument('--required', action='store_true', help='fail when the previous release cannot be read')
    args = parser.parse_args(argv)
    version = args.service_version or manifest_version(ROOT / 'rust/service/Cargo.toml')
    release = args.release or manifest_version(ROOT / 'Cargo.toml')
    fresh = sha256(args.exe.read_bytes())
    try:
        if args.releases:
            releases = json.loads(args.releases.read_text(encoding='utf-8-sig'))
        else:
            releases = fetch_releases(args.repository)
        previous = fetch_previous(releases, release, args.cache)
    except Integrity as error:
        print(f'error: {error}', file=sys.stderr)
        return 1
    except (OSError, ValueError, zipfile.BadZipFile) as error:
        if args.required:
            print(f'error: the previous release could not be read: {error}', file=sys.stderr)
            return 1
        warn(f'the previous release could not be read ({error}); shipping the fresh build of service {version}')
        return 0
    choice = choose(version, fresh, previous)
    print(choice.message)
    if choice.warning:
        warn(choice.warning)
    if choice.reuse:
        part = args.exe.with_name(args.exe.name + '.previous')
        part.write_bytes(previous.data)
        os.replace(part, args.exe)
    return 0


if __name__ == '__main__':
    sys.exit(main())
