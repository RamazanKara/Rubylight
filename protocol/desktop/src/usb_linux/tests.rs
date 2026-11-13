use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::time::Duration;

/// A fake /sys/bus/usb/devices with a webcam at 1-2 that uvcvideo has.
struct Fake {
    root: PathBuf,
}

impl Fake {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("nightjar-usb-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let device = root.join("sys/1-2");
        std::fs::create_dir_all(&device).unwrap();
        std::fs::create_dir_all(root.join("drivers/usb")).unwrap();
        std::fs::create_dir_all(root.join("drivers/usbip-host")).unwrap();
        for (field, value) in [("idVendor", "046D\n"), ("idProduct", "0825\n"), ("serial", " AB 12\n"), ("busnum", "1\n"), ("devnum", "4\n"),
            ("speed", "480\n"), ("bcdDevice", "0010\n"), ("bDeviceClass", "ef\n"), ("bConfigurationValue", "1\n"), ("bNumInterfaces", " 4\n")] {
            std::fs::write(device.join(field), value).unwrap();
        }
        symlink(root.join("drivers/usb"), device.join("driver")).unwrap();
        Self { root }
    }

    fn sysfs(&self) -> PathBuf {
        self.root.join("sys")
    }

    /// Writes an executable script under the fake root.
    fn script(&self, name: &str, text: &str) -> PathBuf {
        let path = self.root.join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The system with `helper` run in place of pkexec and the installed helper.
    fn system(&self, helper: &Path) -> System {
        let installed = self.root.join("installed");
        std::fs::write(&installed, VERSION_MARKER).unwrap();
        System {
            sysfs: self.sysfs(),
            helper: vec!["/bin/sh".into(), helper.into()],
            installed: [installed.clone(), installed.clone(), installed],
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Binds by pointing the driver link at usbip-host, and gives it back when stdin closes.
const FAKE_HELPER: &str = r#"
[ "$1" = hold ] || exit 2
D="$SYSFS/$2"
[ "$3" = "046d:0825:AB12" ] || { echo device_changed >&2; exit 5; }
ln -sfn "$ROOT/drivers/usbip-host" "$D/driver"
echo bound
cat > /dev/null
ln -sfn "$ROOT/drivers/usb" "$D/driver"
echo "unbind complete" >&2
"#;

fn fake_helper(fake: &Fake) -> PathBuf {
    fake.script("helper", &format!("SYSFS={}\nROOT={}\n{FAKE_HELPER}", fake.sysfs().display(), fake.root.display()))
}

#[test]
fn sysfs_describes_the_device_and_who_has_it() {
    let fake = Fake::new("describe");
    let system = fake.system(Path::new("/bin/false"));
    assert_eq!(system.identity("1-2").as_deref(), Some("046d:0825:AB12"), "the helper's form: lowercase, no whitespace");
    assert!(!system.bound("1-2"));
    let described = system.describe("1-2").unwrap();
    assert_eq!((described.vendor, described.product, described.busnum, described.devnum, described.speed), (0x046d, 0x0825, 1, 4, 3));
    assert_eq!((described.class, described.configuration, described.interfaces), (0xef, 1, 4));
    assert!(system.describe("1-9").is_none(), "unplugged");
}

#[test]
fn a_composite_device_names_its_first_interfaces_class() {
    let fake = Fake::new("class");
    std::fs::write(fake.sysfs().join("1-2/bDeviceClass"), "00\n").unwrap();
    std::fs::create_dir_all(fake.sysfs().join("1-2:1.0")).unwrap();
    std::fs::write(fake.sysfs().join("1-2:1.0/bInterfaceClass"), "0e\n").unwrap();
    let system = fake.system(Path::new("/bin/false"));
    let described = system.describe("1-2").unwrap();
    assert_eq!(system.class(&described), 0x0e, "video");
}

#[test]
fn attaching_binds_to_usbip_host_and_closing_gives_the_device_back() {
    let fake = Fake::new("hold");
    let system = fake.system(&fake_helper(&fake));
    let device = system.open("1-2", 0x046d, 0x0825).unwrap();
    assert!(system.bound("1-2"), "usbip-host has it while attached");
    assert_eq!(device.class(), 0xef);
    assert!(device.release(true).unwrap().success());
    assert!(!system.bound("1-2"), "back with its own driver");
    assert!(device.release(true).is_none(), "released once");
}

#[test]
fn dropping_the_device_gives_it_back_as_a_crash_closing_the_pipe_does() {
    let fake = Fake::new("drop");
    let system = fake.system(&fake_helper(&fake));
    let device = system.open("1-2", 0x046d, 0x0825).unwrap();
    assert!(system.bound("1-2"));
    drop(device);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while system.bound("1-2") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!system.bound("1-2"), "the helper unbinds once its stdin closes");
}

#[test]
fn another_device_at_the_port_is_refused_before_the_helper_runs() {
    let fake = Fake::new("other");
    let system = fake.system(&fake.script("never", "touch \"$0.ran\"; exit 0\n"));
    assert_eq!(system.open("1-2", 0x046d, 0x0001).err().as_deref(), Some(tr("The device was unplugged.")));
    assert_eq!(system.open("1-9", 0x046d, 0x0825).err().as_deref(), Some(tr("The device was unplugged.")));
    assert!(!fake.root.join("never.ran").exists());
}

#[test]
fn the_helpers_failures_read_as_what_to_do() {
    let fake = Fake::new("fail");
    for (code, stderr, expected) in [
        (3, "usbip tool not found", Missing::Usbip.message()),
        (5, "device_changed", tr("Another device was plugged in there meanwhile. Press Refresh and attach again.").to_owned()),
        (126, "", tr("Forwarding this device needs your password, and the prompt was cancelled or failed.").to_owned()),
        (127, "Error executing command as another user: No authentication agent found.", tr("No password prompt could be shown: no polkit authentication agent runs in this desktop session.").to_owned()),
        (1, "usbip: error: device on busid 1-2 is already bound to usbip-host\nlast words", trf("Linux’s USB/IP tools failed: {}", &[&"last words"])),
    ] {
        let helper = fake.script(&format!("exit{code}"), &format!("echo '{stderr}' >&2\nexit {code}\n"));
        let system = fake.system(&helper);
        assert_eq!(system.open("1-2", 0x046d, 0x0825).err(), Some(expected), "exit {code}");
        assert!(!system.bound("1-2"));
    }
    let mut system = fake.system(Path::new("/bin/false"));
    system.helper = vec!["/nonexistent/pkexec".into()];
    assert_eq!(system.open("1-2", 0x046d, 0x0825).err(), Some(Missing::Pkexec.message()), "no pkexec");
}

#[test]
fn a_helper_killed_by_its_watchdog_says_the_tools_hung() {
    assert_eq!(failure(None, "operation_timeout\n"), tr("Linux’s USB/IP tools didn’t answer in time. Unplug the device, plug it in again and retry."));
}

#[test]
fn the_connection_is_made_on_loopback_and_its_listener_is_gone_after() {
    let (theirs, ours) = loopback_pair().unwrap();
    assert_eq!(theirs.peer_addr().unwrap(), ours.local_addr().unwrap(), "connected to each other");
    for address in [theirs.local_addr().unwrap(), ours.local_addr().unwrap()] {
        assert!(address.ip().is_loopback(), "{address}");
    }
    // Sockets in state 0A (LISTEN) on the pair's port: none.
    let port = format!(":{:04X} ", theirs.local_addr().unwrap().port());
    let tcp = std::fs::read_to_string("/proc/net/tcp").unwrap();
    let listening: Vec<&str> = tcp.lines().filter(|l| l.split_whitespace().nth(1).is_some_and(|local| format!("{local} ").ends_with(&port)) && l.split_whitespace().nth(3) == Some("0A")).collect();
    assert!(listening.is_empty(), "nothing listens any more: {listening:?}");
}

/// The real helper, every root path pointed into the fake, with fake usbip, usbipd,
/// modprobe and udevadm on its PATH. usbip-host's sysfs files are plain ones here: binding
/// makes the device "in use" (2) at once, as writing usbip_sockfd does in the kernel.
struct Real {
    script: PathBuf,
    /// What usbip ran.
    log: PathBuf,
    /// What modprobe ran.
    modprobe: PathBuf,
}

impl Real {
    fn new(fake: &Fake) -> Self {
        let bin = fake.root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let (root, sys) = (fake.root.display(), fake.sysfs());
        let log = fake.root.join("usbip.log");
        let modprobe = fake.root.join("modprobe.log");
        fake.script("bin/usbip", &format!(
            "echo \"$*\" >> {log}\ncase \"$1\" in\n\
                 bind) [ \"$(readlink {sys}/$3/driver)\" = {root}/drivers/usbip-host ] && {{ echo \"usbip: error: device on busid $3 is already bound to usbip-host\"; exit 1; }}\n\
                 ln -sfn {root}/drivers/usbip-host {sys}/$3/driver; echo 2 > {sys}/$3/usbip_status;;\n\
                 unbind) ln -sfn {root}/drivers/usb {sys}/$3/driver;;\nesac\n",
            log = log.display(), sys = sys.display()));
        fake.script("bin/usbipd", &format!("touch {root}/usbipd.ran\n"));
        fake.script("bin/modprobe", &format!("echo \"$*\" >> {}\n", modprobe.display()));
        fake.script("bin/udevadm", "exit 0\n");
        std::fs::create_dir_all(fake.root.join("module/usbip_host")).unwrap();
        let script = SCRIPT
            .replace("PATH=/usr/sbin:/usr/bin:/sbin:/bin", &format!("PATH={}:/usr/bin:/bin", bin.display()))
            .replace("STATE_DIR=/var/lib/nightjar", &format!("STATE_DIR={root}/state"))
            .replace("SYSFS_USB_DEVICES=/sys/bus/usb/devices", &format!("SYSFS_USB_DEVICES={}", sys.display()))
            .replace("SYSFS_USBIP_HOST=/sys/module/usbip_host", &format!("SYSFS_USBIP_HOST={root}/module/usbip_host"))
            .replace("USBIP_HOST_DRIVER=/sys/bus/usb/drivers/usbip-host", &format!("USBIP_HOST_DRIVER={root}/drivers/usbip-host"))
            .replace("DRIVERS_PROBE=/sys/bus/usb/drivers_probe", &format!("DRIVERS_PROBE={root}/drivers_probe"))
            .replace("HELPER_FILE=/usr", &format!("HELPER_FILE={root}/usr"))
            .replace("POLICY_FILE=/usr", &format!("POLICY_FILE={root}/usr"))
            .replace("UDEV_RULE_FILE=/usr", &format!("UDEV_RULE_FILE={root}/usr"));
        assert!(!script.contains("=/sys/") && !script.contains("=/var/") && !script.contains("=/usr/"), "every root path is the fake's");
        Self { script: fake.script("helper", &script), log, modprobe }
    }

    /// Runs it directly, as `hold` with a loopback connection made here on stdin.
    fn hold(&self, busid: &str, identity: &str) -> (Child, TcpStream) {
        let (theirs, ours) = loopback_pair().unwrap();
        let child = Command::new("/bin/sh").arg(&self.script).args(["hold", busid, identity])
            .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs))).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        (child, ours)
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
        Command::new("/bin/sh").arg(&self.script).args(args).stdin(Stdio::null()).output().unwrap()
    }

    fn ran(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn reloads(&self) -> String {
        std::fs::read_to_string(&self.modprobe).unwrap_or_default()
    }
}

fn wait_for(child: &mut Child) -> std::process::ExitStatus {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.try_wait().unwrap() { return status; }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("the helper did not exit");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn the_helper_script_binds_hands_usbip_host_the_connection_and_unbinds() {
    let fake = Fake::new("script");
    let real = Real::new(&fake);
    let sys = fake.sysfs();
    let system = fake.system(&real.script);
    let device = system.open("1-2", 0x046d, 0x0825).unwrap();
    assert!(system.bound("1-2"));
    assert_eq!(read(sys.join("1-2/usbip_sockfd")), "0\n", "usbip-host got the helper's stdin, the connection");
    assert_eq!(read(fake.root.join("state/usb-bindings")), "1-2\t046d:0825\tAB12\n", "the port's device is recorded for the udev rule");
    std::thread::sleep(Duration::from_millis(700));
    assert!(system.bound("1-2"), "held while the export runs");
    // A port unplugged while forwarded waits for usbip-host to have no other device.
    std::fs::write(fake.root.join("state/usb-reload-pending"), "3-1\n").unwrap();
    // Nightjar's end closes; the kernel ends the export.
    std::fs::write(sys.join("1-2/usbip_status"), "1\n").unwrap();
    assert!(device.release(true).unwrap().success());
    assert!(!system.bound("1-2"));
    assert_eq!(real.ran(), "bind -b 1-2\nunbind -b 1-2\n");
    assert_eq!(read(fake.root.join("drivers_probe")), "1-2\n", "its own driver probes it again");
    assert_eq!(read(fake.root.join("state/usb-bindings")), "", "forgotten");
    assert_eq!(real.reloads(), "-r usbip_host\nusbip_host\n", "the waiting reload runs once this device is given back");
    assert_eq!(read(fake.root.join("state/usb-reload-pending")), "");
    assert!(!fake.root.join("usbipd.ran").exists(), "no usbipd: nothing listens for USB/IP");
}

#[test]
fn the_helper_gives_the_device_back_after_nightjar_closed_its_pipes() {
    let fake = Fake::new("sigpipe");
    let real = Real::new(&fake);
    let (mut child, ours) = real.hold("1-2", "046d:0825:AB12");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line, "bound\n");
    // Nightjar crashed: its ends of stdout and stderr are gone, and so is its connection.
    drop(child.stderr.take());
    drop(ours);
    std::fs::write(fake.sysfs().join("1-2/usbip_status"), "1\n").unwrap();
    assert!(wait_for(&mut child).success(), "a write to the closed stderr does not kill it");
    assert_eq!(real.ran(), "bind -b 1-2\nunbind -b 1-2\n");
    assert_eq!(read(fake.root.join("drivers_probe")), "1-2\n", "its own driver probes it again");
    assert_eq!(read(fake.root.join("state/usb-bindings")), "", "forgotten");
}

#[test]
fn the_helper_told_to_stop_ends_the_export_and_gives_the_device_back() {
    let fake = Fake::new("term");
    let real = Real::new(&fake);
    let sys = fake.sysfs();
    let system = fake.system(&real.script);
    let device = system.open("1-2", 0x046d, 0x0825).unwrap();
    let pid = device.held.lock().unwrap().as_ref().unwrap().id();
    assert!(Command::new("kill").args(["-TERM", &pid.to_string()]).status().unwrap().success());
    // The kernel's part: -1 on usbip_sockfd ends the export.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while read(sys.join("1-2/usbip_sockfd")) != "-1\n" {
        assert!(std::time::Instant::now() < deadline, "the helper did not end the export");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::fs::write(sys.join("1-2/usbip_status"), "1\n").unwrap();
    assert!(device.release(true).unwrap().success());
    assert!(!system.bound("1-2"), "back with its own driver");
}

#[test]
fn a_device_exported_already_is_left_alone() {
    let fake = Fake::new("busy");
    let real = Real::new(&fake);
    let sys = fake.sysfs();
    symlink(fake.root.join("drivers/usbip-host"), sys.join("1-2/driver.new")).unwrap();
    std::fs::rename(sys.join("1-2/driver.new"), sys.join("1-2/driver")).unwrap();
    std::fs::write(sys.join("1-2/usbip_status"), "2\n").unwrap();
    let system = fake.system(&real.script);
    assert_eq!(system.open("1-2", 0x046d, 0x0825).err(), Some(busy()), "refused before the password prompt");
    let (mut child, _ours) = real.hold("1-2", "046d:0825:AB12");
    assert_eq!(wait_for(&mut child).code(), Some(10), "the helper refuses it too");
    assert_eq!(failure(Some(10), "busy\n"), busy());
    assert_eq!(real.ran(), "", "neither bound nor unbound");
    assert_eq!(read(sys.join("1-2/usbip_status")), "2\n", "its export goes on");
    assert!(system.bound("1-2"));
}

#[test]
fn a_failed_export_unbinds_only_what_this_run_bound() {
    let fake = Fake::new("exportfail");
    let real = Real::new(&fake);
    let sys = fake.sysfs();
    // usbip-host refuses the connection.
    std::fs::create_dir(sys.join("1-2/usbip_sockfd")).unwrap();
    let (mut child, _ours) = real.hold("1-2", "046d:0825:AB12");
    assert_eq!(wait_for(&mut child).code(), Some(9));
    assert_eq!(real.ran(), "bind -b 1-2\nunbind -b 1-2\n", "bound here: given back");
    // Bound by another program, not exported: theirs to keep.
    std::fs::remove_file(&real.log).unwrap();
    std::fs::remove_file(sys.join("1-2/driver")).unwrap();
    symlink(fake.root.join("drivers/usbip-host"), sys.join("1-2/driver")).unwrap();
    std::fs::write(sys.join("1-2/usbip_status"), "1\n").unwrap();
    let (mut child, _ours) = real.hold("1-2", "046d:0825:AB12");
    assert_eq!(wait_for(&mut child).code(), Some(9));
    assert_eq!(real.ran(), "bind -b 1-2\n", "not unbound");
    assert!(fake.system(&real.script).bound("1-2"));
}

#[test]
fn a_serial_beyond_ascii_binds() {
    let fake = Fake::new("utf8");
    let real = Real::new(&fake);
    std::fs::write(fake.sysfs().join("1-2/serial"), "Kamera-Ü1\n").unwrap();
    let system = fake.system(&real.script);
    assert_eq!(system.identity("1-2").as_deref(), Some("046d:0825:Kamera-Ü1"));
    let device = system.open("1-2", 0x046d, 0x0825).unwrap();
    assert!(system.bound("1-2"));
    std::fs::write(fake.sysfs().join("1-2/usbip_status"), "1\n").unwrap();
    assert!(device.release(true).unwrap().success());
    let (mut child, _ours) = real.hold("1-2", "046d:0825:Kamera\u{1b}1");
    assert_eq!(wait_for(&mut child).code(), Some(2), "control characters are refused");
}

#[test]
fn an_unplug_reloads_usbip_host_only_once_it_has_no_other_device() {
    let fake = Fake::new("unplug");
    let real = Real::new(&fake);
    let sys = fake.sysfs();
    let state = fake.root.join("state");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("usb-bindings"), "1-2\t046d:0825\tAB12\n").unwrap();
    // 1-2 forwarded and unplugged, while another program has 1-5 on usbip-host.
    std::fs::remove_dir_all(sys.join("1-2")).unwrap();
    std::fs::create_dir_all(sys.join("1-5")).unwrap();
    std::fs::write(sys.join("1-5/usbip_status"), "1\n").unwrap();
    symlink(sys.join("1-5"), fake.root.join("drivers/usbip-host/1-5")).unwrap();
    assert!(real.run(&["auto-release", "1-2"]).status.success());
    assert_eq!(real.reloads(), "", "1-5 stays bound");
    assert_eq!(read(state.join("usb-reload-pending")), "1-2\n", "1-2 waits");
    assert_eq!(read(state.join("usb-bindings")), "1-2\t046d:0825\tAB12\n", "and its record stays");
    // 1-5 is let go; a new device at 1-2 is exported: still waiting.
    std::fs::remove_file(fake.root.join("drivers/usbip-host/1-5")).unwrap();
    std::fs::create_dir_all(sys.join("1-2")).unwrap();
    std::fs::write(sys.join("1-2/usbip_status"), "2\n").unwrap();
    symlink(sys.join("1-2"), fake.root.join("drivers/usbip-host/1-2")).unwrap();
    assert!(real.run(&["auto-release", "3-4"]).status.success());
    assert_eq!(real.reloads(), "", "1-2's new device is exported");
    // Only the waiting port's own unexported device is left: any unplug reloads now.
    std::fs::write(sys.join("1-2/usbip_status"), "1\n").unwrap();
    assert!(real.run(&["auto-release", "3-4"]).status.success());
    assert_eq!(real.reloads(), "-r usbip_host\nusbip_host\n");
    assert_eq!(read(state.join("usb-reload-pending")), "");
    assert_eq!(read(state.join("usb-bindings")), "", "forgotten");
    // An unplug of a port nobody forwarded touches nothing.
    assert!(real.run(&["auto-release", "3-4"]).status.success());
    assert_eq!(real.reloads(), "-r usbip_host\nusbip_host\n");
}

#[test]
fn the_helper_holds_only_a_loopback_connection_of_the_caller() {
    let fake = Fake::new("peer");
    let real = Real::new(&fake);
    let (theirs, _ours) = std::os::unix::net::UnixStream::pair().unwrap();
    let output = Command::new("/bin/sh").arg(&real.script).args(["hold", "1-2", "046d:0825:AB12"])
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs))).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "a local socket that is not TCP");
    assert!(String::from_utf8_lossy(&output.stderr).contains("not the caller's loopback connection"));
    let (theirs, _ours) = loopback_pair().unwrap();
    let output = Command::new("/bin/sh").arg(&real.script).args(["hold", "1-2", "046d:0825:AB12"]).env("PKEXEC_UID", "4242424")
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(theirs))).output().unwrap();
    assert_eq!(output.status.code(), Some(2), "a connection another user made");
    assert_eq!(real.ran(), "", "nothing bound");
}

#[test]
fn what_the_install_creates_is_not_writable_by_others() {
    let fake = Fake::new("install");
    let real = Real::new(&fake);
    let output = Command::new("/bin/sh").arg("-c").arg("umask 000; exec /bin/sh \"$0\" install").arg(&real.script).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    for dir in ["usr/lib/nightjar", "usr/share/polkit-1/actions", "usr/lib/udev/rules.d"] {
        assert_eq!(mode(&fake.root.join(dir)), 0o755, "{dir}");
    }
    assert_eq!(mode(&fake.root.join(&POLICY[1..])), 0o644);
    assert_eq!(mode(&fake.root.join(&RULE[1..])), 0o644);
    assert_eq!(mode(&fake.root.join(&HELPER[1..])), 0o755);
}

#[test]
fn the_helper_never_starts_a_listener() {
    let code = SCRIPT.lines().map(|l| l.split('#').next().unwrap_or("")).collect::<Vec<_>>().join("\n");
    for listener in ["usbipd", "nc ", "socat", "listen"] {
        assert!(!code.contains(listener), "{listener}");
    }
}

#[test]
fn the_helper_holds_only_with_a_connection_on_stdin() {
    let output = Command::new("/bin/sh").arg("-c").arg(SCRIPT).arg("nightjar-usb-helper").args(["hold", "1-2", "x"]).stdin(Stdio::null()).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("stdin is not a socket"));
}

#[test]
fn the_helper_refuses_a_busid_that_is_not_one() {
    let output = Command::new("/bin/sh").arg("-c").arg(SCRIPT).arg("nightjar-usb-helper").args(["hold", "1-2;reboot", "x"]).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
}

/// The packages install the policy and the rule from these files; the helper's install
/// writes the same.
#[test]
fn the_packaged_policy_and_rule_are_the_ones_the_helper_installs() {
    let policy = include_str!("../../packaging/linux/usb/io.github.ramazankara.nightjar.usb.policy");
    let rule = include_str!("../../packaging/linux/usb/90-nightjar-usb.rules");
    assert!(SCRIPT.contains(&format!("<<'POLICY'\n{policy}POLICY\n")));
    assert!(SCRIPT.contains(&format!("<<'RULE'\n{rule}RULE\n")));
    assert!(SCRIPT.contains(VERSION_MARKER));
    assert!(policy.contains(HELPER) && rule.contains(HELPER) && SCRIPT.contains(&format!("HELPER_FILE={HELPER}")));
    assert!(SCRIPT.contains(&format!("POLICY_FILE={POLICY}")) && SCRIPT.contains(&format!("UDEV_RULE_FILE={RULE}")));
    let syntax = Command::new("/bin/sh").arg("-n").arg("-c").arg(SCRIPT).status().unwrap();
    assert!(syntax.success());
}
