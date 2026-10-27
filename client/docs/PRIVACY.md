# Nightjar privacy

Describes the Nightjar for Android source prepared for 0.4.6.

Nightjar streams your PC's desktop, apps, video and audio directly to your
Android device. Your controls and any clipboard text you choose to send go to
your PC. Nightjar does not operate a streaming relay, advertising service,
analytics service or automatic crash-report upload service.

## What stays on your device

Nightjar stores saved and paired hosts, including their names, addresses,
identifiers and host certificates. It also stores the client certificate and
private key used for pairing, a locally generated client identifier, app and
per-PC settings, controller mappings, and cached app lists and artwork.

If you list a home Wi-Fi for named profiles, Nightjar stores that Wi-Fi name
on this device to tell your home Wi-Fi from other Wi-Fi when a stream starts.
Android shows the name of the Wi-Fi you are on only to apps with location
access, so Nightjar asks for location access when you add the Wi-Fi you are
on, after explaining why. Nightjar reads no location with it, reads the Wi-Fi
name only on the Profiles screen and when a stream starts while a home Wi-Fi is
listed, and never sends, logs or includes Wi-Fi names in reports. Without the
access, every Wi-Fi counts as other Wi-Fi. Turn it off in Android's app
settings at any time, and remove listed names under **Settings → Video →
Profiles**.

A short, redacted event log is kept in memory. A redacted record of the latest
crash or abnormal exit is saved locally. Frame-timing measurements are saved in
a local CSV file that you can export. Preparing a problem report creates a temporary
local file. These records can include the app version, Android version, device
model, decoder settings, timing measurements and crash details.

During a stream, Nightjar reads this device's Wi-Fi band, channel, signal
strength, link speed and Wi-Fi standard for the Debug overlay and the stream
health report. It does not read the network name or access point address. The
reading stays in memory and is sent nowhere; **Copy** in the stream health
report includes it in the text you copy.

## Network connections

Stream content, input and clipboard transfers are between your device and the
PC you connect to. When you turn on **Send microphone to the PC**, Nightjar asks
for microphone access and, during a stream, sends your microphone, encrypted, to
that PC only. It is not recorded or stored on your device, and it stops when you
mute it in the stream menu or the stream ends. Pairing exchanges identity information with that PC. Host
discovery also uses your local network.

If you enable **Send hardware metrics to client** on a host that supports it
(currently Rubylight), Nightjar can request host CPU usage and available AMD GPU readings, including
GPU names, utilization, clocks, temperatures, power, fans and VRAM use. This
uses the paired streaming HTTPS connection during this device's active video
stream while its foreground performance overlay is visible. Polling stops
when the overlay is hidden, the app is paused or in picture in picture, or the
stream ends. These readings are held in memory for the overlay and are not
sent to an analytics service. **Copy stats** includes available hardware
readings and GPU names in the text you choose to copy or share.

The locally generated client identifier is sent with every host HTTP request,
including polling an unpaired host over unencrypted HTTP. Hosts can use this
identifier to recognize this installation across requests.

The current app also makes public network checks. It can contact
`stun.moonlight-stream.org` to find an external network address and
`android.conntest.moonlight-stream.org` to diagnose connection failures. Those
servers receive your public IP address and network protocol traffic. They do
not receive your stream, clipboard or problem reports. Their retention
practices are not established by this repository.

Opening a help or source link contacts the website you choose to visit. The
site and your browser apply their own privacy practices.

## Sharing a problem or crash report

Nightjar does not automatically send problem or crash reports. A report is
shared only after you choose Share and select a receiving app in Android's
share sheet. Canceling the sheet does not share the report. The report omits
addresses, user-assigned host names, PINs, credentials and free-text diagnostic
details. The receiving app controls what happens to the report afterward.

The latency benchmark's **Share results** works the same way: nothing is sent
unless you copy or share the text yourself. The text lists the device's
manufacturer, model and codename, SoC maker and model, hardware and board
names, GPU name, Android version and API level, the Nightjar version, the
decoder names, timings and low-latency switches from the decoder benchmark,
and the display, audio and timing figures of the latest latency test. It leaves
out PC names, addresses, serial numbers, the build fingerprint, account data
and anything about your streams. See
[device results](devices.md#what-is-left-out).

**Open report form** copies that text and opens the public GitHub issue form
for device results in your browser. The link carries no device data, and the
form holds none until you paste the text. If you post it, the issue is public on
GitHub under your account, and the maintainers copy the results into the
public device table on the Nightjar website. GitHub's own privacy practices
apply to the issue. Nightjar makes no network request for this.

## Backups and deletion

Android backup and device transfer are enabled. Depending on your Android
settings, eligible app data, including the saved-host database and local
latency files, may be backed up or transferred by Android. Client certificates,
private keys, the client identifier and preferences are excluded. Crash records
and temporary report files are kept outside Android backup.

Remove a saved PC in Nightjar to remove its saved-host entry, per-PC and per-game settings, hidden-app choices,
cached app list and artwork. Revoke pairing
on the PC to remove that device's access there. Clear Nightjar's app storage
or uninstall it to remove its local data. Separately delete any exported logs,
shared reports or Android backups using the app or service holding them.

Nightjar has no developer-hosted user account to delete. For questions about
this policy, use the contact details on the Nightjar store listing.
