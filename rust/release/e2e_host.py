"""Isolated release fixture setup, shared by e2e and the soak runner."""
import hashlib, json, os, pathlib, shutil, subprocess, sys
import xml.etree.ElementTree as ET
import requests
from e2e_result import active_clients, display_refresh

INSTALLED_LOG = pathlib.Path(r'C:\ProgramData\Butterpollo\config\logs\butterpollo.log')


def stage_fault_host(package, binary, directory):
    # Opus is loaded beside the executable, so PATH alone cannot supply the
    # debug host's runtime. Keep the staged copy inside this test's artifacts.
    directory.mkdir()
    staged = directory / binary.name
    shutil.copy2(binary, staged)
    for library in package.glob('*.dll'):
        shutil.copy2(library, directory / library.name)
    return staged


def installed_idle():
    assert not active_clients(INSTALLED_LOG.read_text(errors='replace')), 'the installed host log has an active stream'
    plain = requests.Session(); plain.trust_env = False
    info = ET.fromstring(plain.get('http://127.0.0.1:47989/serverinfo', timeout=3).text)
    assert info.findtext('state') == 'SUNSHINE_SERVER_FREE', 'the installed host is streaming'
    assert info.findtext('RustHostSessionCount') in (None, '0'), 'the installed host has a session'
    assert info.findtext('RustHostPendingSessionCount') in (None, '0'), 'the installed host has a pending session'
    assert info.findtext('RustHostApplicationActive') in (None, '0'), 'the installed host runs an app'


# Below this the tone the host captures from the sink is near silent.
QUIET_SINK = .1


def endpoint_volume(device_id):
    """The playback endpoint's master volume (0-1) and mute, read only:
    nothing here sets a volume or any other device setting."""
    import ctypes
    from ctypes import HRESULT, POINTER, WINFUNCTYPE, byref, c_float, c_int, c_void_p, c_wchar_p

    class GUID(ctypes.Structure):
        _fields_ = [('data1', ctypes.c_uint32), ('data2', ctypes.c_uint16), ('data3', ctypes.c_uint16), ('data4', ctypes.c_ubyte * 8)]

    ole = ctypes.OleDLL('ole32')
    def guid(text):
        value = GUID(); ole.IIDFromString(c_wchar_p('{' + text + '}'), byref(value)); return value
    def call(obj, slot, *types):
        return WINFUNCTYPE(HRESULT, c_void_p, *types)(ctypes.cast(obj, POINTER(POINTER(c_void_p)))[0][slot])
    def release(obj):
        if obj: WINFUNCTYPE(ctypes.c_ulong, c_void_p)(ctypes.cast(obj, POINTER(POINTER(c_void_p)))[0][2])(obj)
    try:
        ole.CoInitializeEx(None, 0)
        initialised = True
    except OSError:  # COM already initialised in another mode on this thread
        initialised = False
    enumerator, device, volume = c_void_p(), c_void_p(), c_void_p()
    try:
        ole.CoCreateInstance(byref(guid('BCDE0395-E52F-467C-8E3D-C4579291692E')), None, 1,
                             byref(guid('A95664D2-9614-4F35-A746-DE8DB63617E6')), byref(enumerator))
        call(enumerator, 5, c_wchar_p, POINTER(c_void_p))(enumerator, device_id, byref(device))  # GetDevice
        call(device, 3, POINTER(GUID), ctypes.c_uint32, c_void_p, POINTER(c_void_p))(
            device, byref(guid('5CDF2C82-841E-4546-9722-0CF74078229A')), 0x17, None, byref(volume))  # Activate
        level, muted = c_float(), c_int()
        call(volume, 9, POINTER(c_float))(volume, byref(level))  # GetMasterVolumeLevelScalar
        call(volume, 15, POINTER(c_int))(volume, byref(muted))  # GetMute
        return round(level.value, 3), bool(muted.value)
    finally:
        for obj in (volume, device, enumerator):
            release(obj)
        if initialised:
            ole.CoUninitialize()


def sink_warning(name, volume, muted, error=None):
    """A loud line for a sink the tone check would hear as silence. The
    volume is the user's to set: the check only reports it."""
    if error:
        return f'could not read the volume of {name}: {error}'
    if muted:
        return f'{name} is muted: the audio tone check hears silence. Unmute it in Windows sound settings.'
    if volume < QUIET_SINK:
        return f'{name} is at {round(volume * 100)} % volume: the audio tone check may hear silence. Raise it in Windows sound settings.'
    return None


def prepare(args, case, port=48523, rust_log='info'):
    profile = case / 'config'; (profile / 'logs').mkdir(parents=True)
    env = os.environ.copy(); env.update(PATH=str(args.package) + ';' + env['PATH'], RUST_LOG=os.environ.get('BUTTERPOLLO_E2E_RUST_LOG', rust_log), NO_PROXY='*')
    host_exe = args.package / 'butterpollo.exe'
    diag = json.loads(subprocess.check_output([str(host_exe), '--diagnostics'], env=env, text=True))
    monitor = next(m for m in diag['monitors'] if m['primary'])
    display = next(d for d in diag['displays'] if d['display_name'] == monitor['display_name'])
    display = dict(display, refresh_hz=display_refresh(display['display_name']), hdr=monitor['hdr_enabled'])
    if args.mode is None:
        args.mode = f"{round(display['width'] * 720 / display['height'] / 2) * 2}x720x60"
    width, height, fps = args.mode.split('x')
    audio_probe = args.client.parent / 'audio_probe.exe'
    motion_probe = args.client.parent / 'motion_probe.exe'
    assert audio_probe.is_file() and motion_probe.is_file(), 'build the audio and motion probes beside the receiver'
    endpoints = json.loads(subprocess.check_output([str(audio_probe), '--list'], env=env, text=True))
    sink = next((e for e in endpoints if e['virtual_sink']), None)
    assert sink, 'Steam Streaming Speakers is required for the isolated audio test'
    try:
        volume, muted = endpoint_volume(sink['id'])
        error = None
    except OSError as failure:
        volume, muted, error = None, None, str(failure)
    sink = dict(sink, volume=volume, muted=muted, warning=sink_warning(sink['name'], volume, muted, error))
    if sink['warning']:
        print(f"WARNING: {sink['warning']}", file=sys.stderr)
    defaults = [
        f'port = {port}', 'encoder = amf', 'capture = wgc', f"output_name = {monitor['device_id']}",
        f'minimum_fps_target = {fps}', 'virtual_display_mode = disabled', 'dd_configuration_option = disabled',
        'dd_resolution_option = disabled', 'dd_refresh_rate_option = disabled', 'dd_hdr_option = disabled',
        'dd_always_restore_from_golden = false', 'dd_config_revert_on_disconnect = false',
        'frame_limiter_enable = false', 'install_steam_audio_drivers = false', 'stream_audio = true',
        'audio_sink_capture_only = true', 'auto_capture_sink = false', f"audio_sink = {sink['id']}",
        'keep_sink_default = false', 'upnp = false', 'enable_discovery = false', 'vulkan_hdr_layer = false',
        'system_tray = false', 'update_check_interval = 0', f'pyrowave = {str(args.codec.startswith("pyrowave")).lower()}']
    if args.codec.startswith('pyrowave'):
        defaults.append('lan_encryption_mode = 2')
    # A --config line replaces the default line of the same key.
    key = lambda line: line.split('=', 1)[0].strip()
    overrides = [f'{key(line)} = {line.split("=", 1)[1].strip()}' for line in args.config]
    overridden = {key(line) for line in args.config}
    (profile / 'sunshine.conf').write_text('\n'.join(
        [line for line in defaults if key(line) not in overridden] + overrides + ['']))
    (profile / 'apps.json').write_text(json.dumps({'apps': [{'name': 'Desktop', 'cmd': '', 'virtual-display': False}]}))
    # interop.py's fixture account, in the previous host's format.
    salt = os.urandom(8).hex()
    (profile / 'sunshine_state.json').write_text(json.dumps({'username': 'test', 'salt': salt, 'password': hashlib.sha256(('rust-smoke-only' + salt).encode()).digest()[::-1].hex().upper()}))
    return env, display, sink


def receiver_path(path):
    """The receiver's PATH, with the host package last. The package ships a
    minimal FFmpeg (AC3 and E-AC3 only) that would shadow the full one the
    receiver was built with; the package's other libraries still load."""
    entries = [entry for entry in path.split(os.pathsep) if entry]
    package = [entry for entry in entries if (pathlib.Path(entry) / 'butterpollo.exe').is_file()]
    return os.pathsep.join([entry for entry in entries if entry not in package] + package)


def receiver_environment(env, display, mode, client, vrr=False, port=48523):
    width, height, fps = mode.split('x')
    scale = min(int(width) / display['width'], int(height) / display['height'])
    # Match the encoder's centred letterbox without changing the desktop mode.
    content_width = min(int(width), max(2, int(display['width'] * scale + .5) & ~1))
    content_height = min(int(height), max(2, int(display['height'] * scale + .5) & ~1))
    left = ((int(width) - content_width) // 2) & ~1
    bottom = int(height) - content_height - (((int(height) - content_height) // 2) & ~1)
    client_env = env.copy()
    client_env['PATH'] = receiver_path(env['PATH'])
    client_env.update(BUTTERPOLLO_TEST_HOST='127.0.0.1', BUTTERPOLLO_TEST_PORT=str(port),
                      BUTTERPOLLO_TEST_CLIENT_EXE=str(client), BUTTERPOLLO_TEST_MATCH_DISPLAY='1',
                      BUTTERPOLLO_TEST_REQUIRE_PICTURE='1', BUTTERPOLLO_TEST_WARMUP_SECONDS='3',
                      BUTTERPOLLO_TEST_MIN_FPS=str(float(fps) * .97), BUTTERPOLLO_TEST_AUDIO_TONE='1',
                      BUTTERPOLLO_TEST_REQUIRE_MOTION='1', BUTTERPOLLO_TEST_BARCODE_BOTTOM='1',
                      BUTTERPOLLO_TEST_BARCODE_LEFT=str(left), BUTTERPOLLO_TEST_BARCODE_BOTTOM_MARGIN=str(bottom),
                      BUTTERPOLLO_TEST_BARCODE_SCALE=str(scale), BUTTERPOLLO_TEST_VRR='1' if vrr else '0')
    return client_env
