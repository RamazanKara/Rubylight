# Background streaming in a Windows child session (Experimental)

A device can play on a desktop of its own while you keep using the PC. On
Windows Pro and higher, Rubylight can run a Windows **child session**: a second
sign-in of your own account inside your session, with its own desktop, its own
mouse and keyboard focus and its own apps. A second Rubylight host runs inside
it on its own ports, and clients pair with it like with any host. The way it
works is ported from [Annex](https://github.com/mattxslv/annex) (MIT).

It is **Experimental** and off by default. The isolated virtual display
([Background streaming](configuration.md#background-streaming)) stays the
default, and it is what Windows Home uses: Home has no child sessions.

## Turning it on

1. In the console, set **Background mode** to **Child session** (or **Automatic**,
   which uses child sessions where the Windows edition has them). The setting
   is `background_mode = auto | child_session | virtual_display`. The console
   and the host log say which mode runs and why.
2. Under **Maintenance**, open **Background child session (Experimental)** and
   choose **Set up child sessions**. Setup:
   - turns on Windows child sessions;
   - turns on Remote Desktop for this PC only: Windows' own Remote Desktop
     firewall rules stay off and a Rubylight block rule keeps the RDP port,
     and Remote Desktop over WebSocket's ports (3387 and 3392), closed to the
     network. Setup refuses when Windows Firewall is off for a
     network profile, or when group policy overrides the local firewall rules.
     RDP is turned on only if the rules in effect (local rules merged with
     group policy) still block it: the block rule is in force for TCP and UDP,
     closes exactly the RDP port setup recorded (RDP must still listen on that
     port) and the WebSocket ports, on every network profile and with no
     other condition (remote port, address, program, service, interface),
     and no Remote Desktop allow rule is in force, nor any allow rule that
     overrides block rules (authenticated bypass) for one of those ports.
     When RDP moves to another port, the
     check refuses until you set up child sessions again, which follows the
     new port; the port is read both from the registry and from the port the
     Remote Desktop service really listens on, and setup refuses while the
     two differ (restart the PC first). When the Remote Desktop service
     shares its process with other Windows services, only the sockets that
     process opened for Remote Desktop count. That check runs at setup, again
     at every host start and before every child-session start. When it fails
     after setup, or cannot run at all (tried three times, 5 s apart, as
     while Windows starts; or Rubylight's setup record cannot be read),
     Rubylight turns Remote Desktop off at once
     (`fDenyTSConnections = 1`), reads that back, says so loudly in the
     console and the log, and refuses to start the child session until you
     fix the firewall and set up child sessions again. A group policy that
     keeps Remote Desktop on (`fDenyTSConnections = 0` under
     `HKLM\SOFTWARE\Policies\Microsoft\Windows NT\Terminal Services`) wins
     over that, so Rubylight reports it as a failure to turn RDP off;
   - raises the session frame rate (`DWMFRAMEINTERVAL = 8`, about 120 fps);
   - lets the Remote Desktop client keep drawing while minimized
     (`RemoteDesktop_SuppressWhenMinimized = 2`);
   - records every original value first, in one record for the whole PC
     (`%ProgramData%\Butterpollo\windows-settings\child_session_setup.json`,
     which only SYSTEM and Administrators can change and anyone can read;
     Rubylight refuses a record anyone else owns), whichever Rubylight host
     set it up. A host without administrator rights (a portable one) can't
     check the firewall or turn Remote Desktop off, so it starts no child
     session and says why in its log; the service, or a host run as
     administrator, does. Setup
     refuses on a PC another Rubylight host set up without that record:
     restore it there first. Only the main host sets up and restores, in
     child-session mode; the child session's own host never does.
     **Restore Windows settings** puts them all back, and so does uninstalling
     Rubylight. When the uninstaller's restore fails or cannot run (the host
     is missing, does not start, times out or crashes), the uninstaller turns
     Remote Desktop off itself (`fDenyTSConnections = 1`, read back, and no
     group policy keeping it on), says so in its note, keeps the record and
     exits with code 2.
3. Enter your **Windows password** (not your PIN) in the panel and start the
   child session. The password is stored for your account only, encrypted with
   Windows DPAPI, and never logged; **Forget saved password** deletes it.

A small viewer runs in your session and keeps the child session attached (it
only has a display while it is attached) through the built-in Remote Desktop
client over loopback (`127.0.0.1`), on the RDP port setup recorded. The
loopback server's certificate is self-signed, so the client does not
authenticate it; instead, right before every connect and reconnect the viewer
checks that the Remote Desktop service listens on that port, that no other
program listens on it (TCP or UDP, IPv4 or IPv6), and that RDP is still on, on
that port. The client's own automatic reconnect is off, so every reconnect
goes through that check. On any mismatch it refuses, says why in the panel
and the log, and does not retry.

What the check cannot rule out: a program that binds the RDP port on
`127.0.0.1` between the check and the connect, while running as the Remote
Desktop service's own account (Network Service), as SYSTEM or as an elevated
administrator. The client would then send your password to that program.
Windows refuses that bind to other accounts. Measured on this PC (Windows 11
26200): from an administrator's unelevated token, which has a standard user's
rights, binding `127.0.0.1`, the IPv4-mapped `::ffff:127.0.0.1` or the
wildcard address with `SO_REUSEADDR` on a port another account's service
listens on failed every time with `WSAEACCES` (10013). The same bind from the
listening socket's own account succeeded and took the loopback connections.
A program running with those rights can already change Remote Desktop's
settings itself. The viewer does not pin the server's certificate.
Windows shortcuts stay on
your own desktop. The panel
starts and stops the child session and its host, sets its resolution and scale
(`child_session_resolution`, `child_session_scale`; 1920x1080 at 100 % by
default), reconnects it and signs it out.

## Limits

- **No HDR** in the child session.
- The RDP compositor caps the child session at **about 120 fps**.
- Windows may render the child session on an **integrated GPU**. The child
  session's host detects that and says so loudly in the console and the log,
  and never falls back to encoding in software. To fix it, set the GPU
  preference for `butterpollo.exe` to the discrete GPU in Windows' graphics
  settings, or enable the group policy "Use hardware graphics adapter for all
  Remote Desktop Services sessions".
- One child session per Windows account, and Windows Pro, Enterprise or
  Education. **Windows Home keeps the virtual-display mode**, shown in the
  console as "Background streaming (virtual display): Windows Home".
- The child session's host runs as administrator, so games that run as
  administrator get its input. Its console uses the main console's sign-in.

## Status

Ported and covered by unit tests with fake Windows settings (setup and restore,
the session state machine, the firewall and registry changes). The hardware
run needs your Windows password, which no agent types, so it waits for a user
run; until then the feature stays Experimental. See the
[hardware matrix](hardware-matrix.md).
