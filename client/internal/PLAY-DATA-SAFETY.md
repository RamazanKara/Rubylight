# Play Data safety and content rating — draft

For Nightjar, drafted for 0.4.0 (`versionCode` 316) when the app was still called
Rubylight Android. These are proposed answers
from a source review, not a submitted declaration. Review the final release
and the live questionnaire before submission.

## Data safety

The current build cannot support an unconditional “no data leaves the device
except to your PC” answer. `ComputerManagerService.populateExternalAddress()`
uses public STUN. `Game` and `AddComputerManually` call the public connectivity
test server. Android backup is enabled. The server operators' logging and
retention policies are not known from this code.

Google distinguishes local processing, off-device collection and sharing.
Transient off-device processing still belongs in the form. User-requested
sharing can qualify for a sharing exception; it is not a blanket exemption
from collection. See [Google's Data safety guidance](https://support.google.com/googleplay/android-developer/answer/10787469?hl=en).

| Form question | Proposed answer / release review |
| --- | --- |
| Collects or shares required user data types? | Yes, conservatively, pending review of public network checks, host transfers and optional reports. Do not submit a blanket No. |
| All collected data encrypted in transit? | No. Public STUN and connectivity probes are not all encrypted; host protocol support also varies. |
| Account creation methods | None. Pairing with a PC is not a developer-hosted account. |
| Account deletion | Not applicable: no developer-hosted account. |
| Data deletion request mechanism | No in-app server-side deletion mechanism. Local storage can be cleared. Confirm handling of any reports received by the publisher and any network-server records before answering for those records. |
| Independent security review | No review established by this repository. |
| Privacy policy | Publish `docs/PRIVACY.md` at a publicly accessible URL and enter that URL. |

| Data type / flow | Proposed handling answer |
| --- | --- |
| Device or other IDs | The generated client identity goes to the paired PC. Public network checks expose an IP address. Review whether network operators retain or link that address. Conservative draft: collected for app functionality; public checks are not opt-in. Confirm third-party sharing and retention with the operators. |
| App interactions / other actions | Control input and host commands go to the user's PC for app functionality. Required when using remote controls. Review the host protocol's encryption before claiming an end-to-end encryption exception. |
| Other user-generated content | Clipboard text goes to the user's PC only when requested. Optional; app functionality. Review the encrypted transport and receiver before finalizing the collection answer. |
| Crash logs / diagnostics | Local by default. Users may share a redacted report through an app they select. Draft: optional collection for diagnostics if reports reach the publisher; user-requested sharing exception where applicable. No automatic upload. Do not claim ephemeral processing for retained reports. |
| Location | No device location permission or location inference in the app. Confirm whether public network operators infer location from IP addresses. |
| Other personal info, financial info, health, contacts, calendar, browsing history | No collection implemented by Nightjar itself. Streamed desktop content and user-requested clipboard text are handled by the user's chosen host. |
| Photos, videos, audio, files, installed apps | Host media, app lists and artwork are received for local display. No upload of the Android device's media library or installed-app inventory. Optional exported logs are user-controlled files. |
| Audio: voice or sound recordings | Optional, off by default: with **Send microphone to the PC** on and `RECORD_AUDIO` granted, microphone audio is encrypted (AES-128-CBC under the per-launch key) and sent only to the user's PC during a stream; nothing is stored or sent elsewhere. Draft: optional collection for app functionality, transferred to the user's own host, not shared with the publisher. Confirm the answer before submitting. |
| Android backup / transfer | Saved hosts and eligible files may leave the device through Android. Preferences and client pairing credentials are excluded. Confirm the platform-backup treatment in the final form. |

No advertising, marketing, sale of data or automatic analytics SDK was found
in the app dependencies. Do not mark public-server traffic ephemeral without
confirmation of server retention. Confirm whether the network operators are
service providers before applying that sharing exception.

## Content rating questionnaire

Use the remote-desktop / utility description in the questionnaire. The app
streams content selected on the user's own PC; it does not supply a catalog
of games or media. The manifest's game category is not a content rating.

| Question topic | Draft answer |
| --- | --- |
| Category | All other app types / utility, subject to the live category wording. |
| Contact email | Publisher must enter the actual IARC contact email. |
| Violence, blood, fear or horror supplied by the app | No. |
| Sexual content, nudity or suggestive themes supplied by the app | No. |
| Strong language, drugs, alcohol or tobacco supplied by the app | No. |
| Gambling, simulated gambling or contests with cash prizes | No. |
| Purchases, paid digital goods, subscriptions or randomized purchases | No. |
| Ads | No. |
| Built-in chat, social network, public posts or user-to-user content sharing | No. Remote input and optional report sharing do not create a social platform. |
| Shares users' physical location | No. |
| Content accessed from outside the app | Yes: the desktop and apps selected on the user's PC. Disclose remote access if the form asks about external or unrestricted content. |
| General-purpose web browser or search engine | No built-in browser/search product. Help links can open a browser or fallback WebView, and a remote desktop can display a PC browser. Review any broader unrestricted-internet-access question accordingly. |

Do not preselect an age rating or claim a Families commitment. The publisher
must choose the intended audience separately and review the ratings returned
by IARC. [Google's content rating instructions](https://support.google.com/googleplay/android-developer/answer/9859655?hl=en)
explain how questionnaire responses produce the final regional ratings.
