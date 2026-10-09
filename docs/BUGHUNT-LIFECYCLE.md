# Lifecycle bug hunt

Audit date: 2026-10-09. Base: `10ba65e8`, Android worktree only. Changes are uncommitted.

Read `PARITY-MATRIX.md`, `TROUBLESHOOTING.md`, the latest 30 commits, the manifest/build configuration, the lifecycle owners and their Java/JNI teardown paths. Recent PiP/ARR, crash-reporting, first-run, input and decoder changes were included. No host repository changes, device commands, emulator runs or publishing were performed.

## Findings and fixes

Sixteen bugs fixed. High means a crash, UI hang or shutdown failure; medium means incorrect lifecycle behavior or retained resources; low means a limited resource leak. Native findings are based on source ownership/thread analysis and compilation, not claimed device reproductions.

| ID | Severity | Bug and root cause | Minimal fix | Regression evidence |
| --- | --- | --- | --- | --- |
| L01 | High | Opening another computer when dynamic shortcuts are full loops forever on the UI thread. Eviction repeatedly examines the same list snapshot. | Refresh the snapshot after each removal in `ShortcutHelper`. | `ShortcutLifecycleTest.openingAnotherComputerAtTheShortcutLimitEvictsOnlyTheLastRank`: bounded shadow detects repeated removal before the fix. |
| L02 | High | A successful shortcut launch unbinds CMS, then `onStop()` unbinds it again because `managerServiceBound` remains true. Android throws `IllegalArgumentException`. | Clear the binding flag when the callback unbinds. | `ShortcutTrampolineLifecycleTest.successfulShortcutLaunchReleasesItsBindingOnlyOnce`: the second unbind failed before the fix. |
| L03 | Medium | Every shortcut opens a computer database, including UUID shortcuts and invalid input, and never closes it. | Open it only for name lookup and close it in `finally`. | `ShortcutTrampolineLifecycleTest.uuidShortcutDoesNotOpenAnUnusedDatabase`: one unnecessary open before the fix, none after. Name-lookup success/error ownership also inspected. |
| L04 | High | CMS database acquisition checks the reference count and increments separately. Destruction can decrement 1 to 0 and close the database between those operations, after which a poll resurrects the count and uses the closed database. | Serialize acquisition and release on the same monitor. | Source interleaving analysis; the CMS lifecycle test exercises the same reference ownership. No nondeterministic stress test presented as proof of race coverage. |
| L05 | Medium | A polling listener or database operation throwing bypasses manual reference releases, retaining the CMS database after destruction. | Release polling/removal references in `finally`, retaining existing exception propagation and active-poll accounting. | `ComputerManagerLifecycleTest.failingPollListenerStillReleasesTheDatabaseReference`: injected listener failure leaves count 2 instead of 1 before the fix. |
| L06 | High | Android 14+ NSD calls the computer-discovery listener while holding `listenerLock`. The listener can perform STUN/HTTP requests, while activity pause needs the same lock to stop discovery. | Validate callback ownership under the lock, then report the computer outside it. An already-running network request may complete. | `NsdDiscoveryLifecycleTest.stoppingDiscoveryDoesNotWaitForTheHostNetworkRequest`: deterministic blocked listener causes a shutdown timeout before the fix. |
| L07 | Medium | A stopped `NvConnection` still reports that input is permitted. Late callbacks can call the process-global JNI input API after that connection has ended. | Include the existing volatile `stopRequested` flag in `canSendInput()`. | `NvConnectionTest.stoppedConnectionCannotSendInputIntoTheNextNativeSession`: remains input-capable after stop before the fix. |
| L08 | Medium | Stream failure/reconnect dialogs can keep the activity alive after streaming stops. Both Wi-Fi locks were released only by `onDestroy()`, and some stop paths retained `FLAG_KEEP_SCREEN_ON`. | Release both locks and clear the screen flag in idempotent `stopConnection()`. | `GameLifecycleTest.stoppingAStreamReleasesPowerLocksBeforeTheErrorDialogIsClosed`: locks remain held before the fix. |
| L09 | Medium | Activity recreation between the two frontend folder pickers loses the selected host and first folder; the returned result is silently discarded. | Save/restore the host UUID, display name and first folder URI. The exporter only needs those host fields. | `PcViewLifecycleTest.frontendExportRetainsItsHostAndFirstFolderAcrossRecreation`: restored host is null before the fix. |
| L10 | Medium | Growing decode buffers and HDR callbacks retain JNI local array references on attached native threads. Old frame arrays remain reachable even after deleting their global references; repeated HDR changes also accumulate references. | Delete the temporary local reference after creating the frame-buffer global reference and after delivering HDR metadata. | JNI ownership analysis; normal native build for all four ABIs. CheckJNI/memory-growth verification requires a device. |
| L11 | Medium | A delayed focus callback recaptures the pointer after input was disabled, a cursor was shown, or focus was lost. | Recheck capture state, cursor visibility and current window focus when the callback runs. | `PointerCaptureLifecycleTest.delayedFocusCallbackCannotCaptureAfterInputWasDisabled`: two capture requests instead of one before the fix. |
| L12 | High | Root evdev dispatches keyboard events on its reader thread. Special key combinations can modify views, dialogs, activity state and capture state off the main thread. | Marshal the existing keyboard callback to the main looper before handling it. | `GameLifecycleTest.rootKeyboardCallbacksUseTheMainThread`: translator runs on the reader thread before the fix. |
| L13 | Low | Each unplugged root evdev device leaves a joinable native polling thread whose resources are never joined or detached. | Detach each successfully created polling thread while its list entry is still protected. | Source ownership review and four-ABI NDK syntax checks; repeated physical hotplug remains a device check. |
| L14 | Medium | Root evdev reads `grabbing` on polling threads while the main native thread writes it. The writer's list mutex does not synchronize those unlocked readers. | Use C11 `atomic_int` for the shared grab state. | Source memory-model review and four-ABI NDK syntax checks. No new library dependency. |
| L15 | High | Root capture destruction can inspect still-null startup resources, then join the reader after it creates a listening socket and blocks in `accept()`. Process/socket publication can also race destruction. | Publish shutdown/resources through volatile fields and close resources created after shutdown before using them. | Root Java compilation and both orderings inspected for server socket, SU process and accepted socket. SU approval/denial timing requires a rooted device. |
| L16 | Medium | `Game` is `singleTask`, but has no replacement-intent handling. Launching another app/host while an existing stream is reused (notably PiP) leaves the old stream running. | Release input, stop and finish the old activity, then launch a fresh activity with the new intent. Existing connection serialization governs native handoff. | `GameLifecycleTest.aNewLaunchIntentReplacesTheExistingSingleTaskStream`: no replacement launch before the fix. |

## Verification

Eleven new regression cases were written and observed failing before their corresponding production fixes. The first ten were run in two passes: two initial fixtures needed correction for native-library loading and simulated service binding before they reached the intended assertions. The replacement-intent case was then observed failing separately. These setup failures are not counted as bug evidence.

This checkout originally declared JUnit but no Robolectric dependency. Added test-only Robolectric 4.16.1 and Android test resources so real activity/service/shortcut lifecycles can be exercised without an emulator. Tests use API 28, 29 and 34. Robolectric API 36 requires JDK 21; this audit retains the requested JDK 17 and makes no Android 16 runtime-test claim. See [Robolectric compatibility](https://robolectric.org/compatibility_table/).

Required verification uses PowerShell, `JAVA_HOME=C:\jdk17`, Gradle 9.7.1, `--no-daemon --max-workers=2`:

```text
:app:assembleNonRootDebug :app:testNonRootDebugUnitTest :app:lintNonRootDebug
```

Additional verification: `:app:compileRootDebugJavaWithJavac`; NDK 29 `clang -fsyntax-only` for `evdev_reader.c` targeting ARM64, ARMv7, x86 and x86-64 at API 21; `git diff --check`.

Final run: **BUILD SUCCESSFUL**. Debug APK assembly passed; **526 unit tests passed, zero failures/errors/skips**; lint passed with **zero errors and 176 warnings**. Root Java compilation also passed. All four evdev C syntax checks passed. Gradle reports deprecations and the existing warning that release signing is not configured; this run builds debug artifacts only.

Gradle's default `C:\.gradle` / `C:\.android` locations and a shared dependency-cache JAR initially caused access-denied failures. Writable caches and JVM/Android user homes were placed inside this worktree; the installed Gradle distribution was used directly. Generated caches were subsequently moved under ignored `.gradle/lifecycle-home`, and logs under `app/build/reports/lifecycle`. No source directory was blocked, so no fallback-only patch was necessary.

## Audit reasoning and boundaries

- **Application, splash and process death:** checked application/crash-handler installation, delayed GL initialization, recreated activity state, externally returned document results and destroyed-activity guards. `Game` deliberately finishes a restored activity instead of reusing dead native stream/surface state; this policy remains intact. L09 fixes state for an existing library workflow.
- **Activity callbacks and threading:** traced create/resume/pause/stop/destroy, service waiters, UI-posted network results, dialogs, pending handler work, input listeners and cancellation generations. L02, L05, L06, L11, L12 and L15 address specific mismatches between ownership and callback execution. Existing cancellation/interrupt and native connection serialization remain in place.
- **PiP, rotation, multi-window and back stack:** checked ratio construction, auto-enter suppression, disconnect broadcast/pending-intent ownership, multi-window flags, surface destruction, settings back dispatch and WebView back dispatch. L16 handles replacement launch intents. Physical PiP/fold/display transitions remain listed below.
- **Power, services, notifications and permissions:** checked screen/Wi-Fi/multicast lock acquisition/release, controller sensor/haptic teardown, API gates and bound-service unbinding. There is no application-owned `PowerManager.WakeLock`, notification pipeline or foreground-service start here. CMS, discovery and USB are bound services; adding foreground-service types would not fix an existing call path. L08 moves stream power ownership to the stream stop boundary.
- **Numeric/hardware/locale checks:** reviewed dimensions and PiP ratios, refresh-rate matching above 120 Hz, ms/us/ns conversions, `long` timing arithmetic, dp-to-px UI dimensions, API-gated capture/ADPF/ARR calls, locale changes and RTL-sensitive settings controls. No additional reproducible lifecycle arithmetic or locale defect was established. Codec algorithms, packet parsing and controller mappings are separate areas; their resource-owning interfaces were traced here.
- **JNI/C:** traced Java/native connection start, cancellation, stop, callbacks, attach/detach, global/local references, decoder/audio cleanup and root native threads. The common-C submodule was read but not modified. The PyroWave native create/failure/destructor/JNI ownership paths and MediaCodec/ControllerHandler resource boundaries were also checked; this is not a claim of a new full codec, Vulkan or input-mapping audit.
- **Android 14–16:** manifest is minSdk 21, targetSdk 36, compileSdk 37. The stream's predictive-back opt-out is explicit, while library/settings navigation uses opted-in paths. No new background activity start was added to a service. Android 16's large-screen orientation changes explicitly exempt apps categorized as games, as this app is; OEM/user overrides still need checking. Sources: [Android 14 foreground-service requirements](https://developer.android.com/about/versions/14/changes/fgs-types-required), [Android 16 behavior changes](https://developer.android.com/about/versions/16/behavior-changes-16), [PiP lifecycle guidance](https://developer.android.com/develop/ui/views/picture-in-picture), [JNI local-reference lifetime](https://developer.android.com/ndk/guides/jni-tips).

## Suspected or device-only follow-up

These are not reported as reproduced/fixed device behavior:

1. Android 14–16 PiP entry/exit while USB permission, IME, reconnect dialogs, predictive back, lock screen and another shortcut overlap. Verify the new singleTask handoff stops the old stream once and starts the selected host/app.
2. Fold/unfold, external-display movement and desktop/split-screen resizing during an active SurfaceView or TextureView stream. Verify current display mode, viewport, orientation, insets and input coordinates after the transition. Initial display/codec parameters may not cover every OEM display migration.
3. 120/144/165/240 Hz and Android 16 ARR/VRR: measure actual frame cadence and mode selection after entering/exiting PiP and changing displays. JVM tests cannot establish panel behavior.
4. Sustained thermal pressure/ADPF, battery saver, Doze and Samsung/other OEM low-latency Wi-Fi restrictions. Verify kernel lock release on error/reconnect and that controller sensors/haptics stop. The unit test verifies application ownership, not radio firmware or thermal policy.
5. CheckJNI and native heap/reference tracing through increasing frame sizes, repeated HDR toggles and rapid disconnect/reconnect. Confirm teardown under stalled decoder, audio or Vulkan drivers; no native worker/driver execution was possible in the Windows JVM tests.
6. Root flavor on API 21–25: SU approval/denial delayed across activity destruction, disconnect during socket startup, and repeated mouse/keyboard hotplug and grab/ungrab. Verify the publication, atomic-state and detached-thread fixes on actual root implementations.
7. Process death while an external documents provider owns either export picker, including remote/storage-provider URI grants. The saved-state regression covers activity recreation; real provider availability and grant persistence require integration testing.

## Function inventory

The inventory below groups overloads and same-named anonymous callbacks by owner. Each owner's functions were read together with callback registrations, callers and cleanup paths. Constructors and callback bodies are included; inherited framework methods are not re-enumerated. The reasoning categories above apply to every listed method, with concrete defects mapped in L01–L16.

**[ButterpolloApplication.java](../app/src/main/java/com/limelight/ButterpolloApplication.java)**

`attachBaseContext`, `onCreate`.

**[Game.java](../app/src/main/java/com/limelight/Game.java)**

`run`, `onServiceConnected`, `onServiceDisconnected`, `onCreate`, `onCapturedPointer`, `notifyCrash`, `setPreferredOrientationForCurrentDisplay`, `isValidLaunch`, `onConfigurationChanged`, `getPictureInPictureParams`, `updatePipAutoEnter`, `setMetaKeyCaptureState`, `onUserLeaveHint`, `onPictureInPictureRequested`, `onWindowFocusChanged`, `isRefreshRateEqualMatch`, `isRefreshRateGoodMatch`, `shouldIgnoreInsetsForResolution`, `mayReduceRefreshRate`, `prepareDisplayForRendering`, `hideSystemUi`, `onMultiWindowModeChanged`, `onDestroy`, `onResume`, `onPause`, `onStop`, `onBackPressed`, `showStreamDialog`, `showStreamMenu`, `endRemoteSession`, `refreshHostStatus`, `togglePerformanceOverlay`, `togglePerformanceOverlayMode`, `updatePerformanceOverlay`, `copyPerformanceStats`, `initializeTouchContexts`, `cancelTouchInput`, `showTouchModeDialog`, `setOnscreenControlsEnabled`, `showControllerLayoutDialog`, `reconnectStream`, `showReconnectDialog`, `transferClipboard`, `showServerCommands`, `showHostStatus`, `showBitrateDialog`, `supportsAdaptiveBitrate`, `isAdaptiveBitrateEnabled`, `adaptiveBitratePreferences`, `setAdaptiveBitrateEnabled`, `applyAdaptiveBitrate`, `showHostActionError`, `setInputGrabState`, `handleSpecialKeys`, `getModifierState`, `applyKeySpecificModifiers`, `onKeyDown`, `handleKeyDown`, `onKeyUp`, `handleKeyUp`, `onKeyMultiple`, `handleKeyMultiple`, `handleTextInput`, `getTouchContext`, `toggleKeyboard`, `getLiTouchTypeFromEvent`, `getStreamViewRelativeNormalizedXY`, `normalizeValueInRange`, `getPressureOrDistance`, `getRotationDegrees`, `polarToCartesian`, `cartesianToR`, `getStreamViewNormalizedContactArea`, `sendPenEventForPointer`, `convertToolTypeToStylusToolType`, `trySendPenEvent`, `sendTouchEventForPointer`, `trySendTouchEvent`, `handleMotionEvent`, `onGenericMotionEvent`, `updateMousePosition`, `onGenericMotion`, `onTouch`, `stageStarting`, `stageComplete`, `stopConnection`, `stageFailed`, `connectionTerminated`, `connectionStatusUpdate`, `connectionStarted`, `displayMessage`, `launchConfirmationRequired`, `launchConfirmationFinished`, `launchActionCompleted`, `displayTransientMessage`, `rumble`, `rumbleTriggers`, `setHdrMode`, `setMotionEventState`, `setControllerLED`, `setAdaptiveTriggers`, `surfaceChanged`, `startConnectionOnSurface`, `surfaceCreated`, `configureVideoSurface`, `setArrFrameRate`, `surfaceDestroyed`, `destroyVideoSurface`, `onSurfaceTextureAvailable`, `onSurfaceTextureSizeChanged`, `onSurfaceTextureDestroyed`, `onSurfaceTextureUpdated`, `mouseMove`, `mouseButtonEvent`, `mouseVScroll`, `mouseHScroll`, `keyboardEvent`, `sendKeyboardInput`, `releaseKeyboardInput`, `releaseMouseButtons`, `onSystemUiVisibilityChange`, `onPerfUpdate`, `onUsbPermissionPromptStarting`, `onUsbPermissionPromptCompleted`, `onKey`, `onReceive`, `onNewIntent`.

**[PcView.java](../app/src/main/java/com/limelight/PcView.java)**

`onServiceConnected`, `run`, `onServiceDisconnected`, `onConfigurationChanged`, `initializeViews`, `onClick`, `onCreate`, `onSurfaceCreated`, `onSurfaceChanged`, `onDrawFrame`, `completeOnCreate`, `showPairingGuide`, `startComputerUpdates`, `notifyComputerUpdated`, `stopComputerUpdates`, `startFrontendExport`, `pickFolder`, `onActivityResult`, `runFrontendExport`, `onSaveInstanceState`, `onDestroy`, `onResume`, `onPause`, `onStop`, `onCreateContextMenu`, `onContextMenuClosed`, `doPair`, `showOneTimePinDialog`, `cancelPairing`, `doWakeOnLan`, `doUnpair`, `doAppList`, `onContextItemSelected`, `removeComputer`, `updateComputer`, `getAdapterFragmentLayoutId`, `receiveAbsListView`, `onItemClick`, `ComputerObject`, `toString`.

**[AppView.java](../app/src/main/java/com/limelight/AppView.java)**

`onServiceConnected`, `run`, `onServiceDisconnected`, `onConfigurationChanged`, `startComputerUpdates`, `notifyComputerUpdated`, `stopComputerUpdates`, `onCreate`, `updateHiddenApps`, `populateAppGridWithCache`, `loadAppsBlocking`, `onDestroy`, `onResume`, `onPause`, `onCreateContextMenu`, `onContextMenuClosed`, `onContextItemSelected`, `updateUiWithServerinfo`, `updateUiWithAppList`, `getAdapterFragmentLayoutId`, `receiveAbsListView`, `onItemClick`, `AppObject`, `toString`.

**[ShortcutTrampoline.java](../app/src/main/java/com/limelight/ShortcutTrampoline.java)**

`onServiceConnected`, `run`, `notifyComputerUpdated`, `onServiceDisconnected`, `validateInput`, `onCreate`, `readFrontendEntry`, `putIfPresent`, `findCachedApp`, `onStop`.

**[HelpActivity.java](../app/src/main/java/com/limelight/HelpActivity.java)**

`onCreate`, `onBackInvoked`, `onPageStarted`, `onPageFinished`, `refreshBackDispatchState`, `onDestroy`, `onBackPressed`.

**[AddComputerManually.java](../app/src/main/java/com/limelight/preferences/AddComputerManually.java)**

`onServiceConnected`, `onServiceDisconnected`, `isWrongSubnetSiteLocalAddress`, `parseRawUserInputToUri`, `doAddPc`, `run`, `startAddThread`, `joinAddThread`, `onStop`, `onDestroy`, `onCreate`, `onEditorAction`, `onClick`, `handleDoneEvent`.

**[ControllerMappingActivity.java](../app/src/main/java/com/limelight/preferences/ControllerMappingActivity.java)**

`onCreate`, `selectSingleConnectedController`, `select`, `refresh`, `row`, `primaryTextColor`, `accentColor`, `isControllerEvent`, `dispatchKeyEvent`, `chooseTarget`, `targetName`, `buttonName`.

**[StreamSettings.java](../app/src/main/java/com/limelight/preferences/StreamSettings.java)**

`sectionTitle`, `updateBackCallback`, `navigateBack`, `resetAllSettings`, `onCreate`, `onSaveInstanceState`, `onDestroy`, `exportLatencyCsv`, `onActivityResult`, `run`, `onAttachedToWindow`, `onConfigurationChanged`, `onBackPressed`, `stylePreferences`, `stylePreferenceList`, `entryFor`, `updateSummaries`, `onCreateView`, `onResume`, `findPreference`, `forSection`, `filterPreferences`, `updateValueSummaries`, `onPause`, `setValue`, `appendPreferenceEntry`, `addNativeResolutionEntry`, `addNativeResolutionEntries`, `addNativeFrameRateEntry`, `removeValue`, `resetBitrateToDefault`, `updateCodecSummary`, `onPreferenceClick`, `onPreferenceChange`, `reloadSettings`, `showSection`, `onQueryTextSubmit`, `onQueryTextChange`.

**[HostStreamSettings.java](../app/src/main/java/com/limelight/preferences/HostStreamSettings.java)**

`show`, `dp`, `addLabel`, `numberField`, `addNumber`, `addCheck`, `readNumber`.

**[ComputerManagerService.java](../app/src/main/java/com/limelight/computers/ComputerManagerService.java)**

`onServiceConnected`, `onServiceDisconnected`, `runPoll`, `createPollingThread`, `run`, `startPolling`, `waitForReady`, `waitForPollingStopped`, `addComputerBlocking`, `removeComputer`, `stopPolling`, `createAppListPoller`, `getUniqueId`, `getComputer`, `invalidateStateForComputer`, `onUnbind`, `populateExternalAddress`, `createDiscoveryListener`, `notifyComputerAdded`, `notifyDiscoveryFailure`, `addTuple`, `getLocalDatabaseReference`, `releaseLocalDatabaseReference`, `tryPollIp`, `ParallelPollTuple`, `interrupt`, `startParallelPollThread`, `parallelPollPc`, `pollComputer`, `onCreate`, `onAvailable`, `onLost`, `onDestroy`, `onBind`, `ApplistPoller`, `pollNow`, `waitPollingDelay`, `getPollingTuple`, `start`, `stop`, `PollingTuple`, `ReachabilityTuple`.

**[ComputerDatabaseManager.java](../app/src/main/java/com/limelight/computers/ComputerDatabaseManager.java)**

`ComputerDatabaseManager`, `close`, `initializeDb`, `deleteComputer`, `tupleToJson`, `tupleFromJson`, `updateComputer`, `getComputerFromCursor`, `getAllComputers`, `getComputerByName`, `getComputerByUUID`.

**[DiscoveryService.java](../app/src/main/java/com/limelight/discovery/DiscoveryService.java)**

`setListener`, `startDiscovery`, `stopDiscovery`, `getComputerSet`, `onCreate`, `notifyComputerAdded`, `notifyDiscoveryFailure`, `onDestroy`, `onBind`, `onUnbind`.

**[UsbDriverService.java](../app/src/main/java/com/limelight/binding/input/driver/UsbDriverService.java)**

`reportControllerState`, `reportControllerTouch`, `reportControllerMotion`, `reportControllerBattery`, `deviceRemoved`, `deviceAdded`, `onReceive`, `run`, `setListener`, `setStateListener`, `start`, `stop`, `handleUsbDeviceState`, `isRecognizedInputDevice`, `kernelSupportsXboxOne`, `kernelSupportsXbox360W`, `shouldClaimDevice`, `onCreate`, `onDestroy`, `onUnbind`, `onBind`, `onUsbPermissionPromptStarting`, `onUsbPermissionPromptCompleted`.

**[PosterContentProvider.java](../app/src/main/java/com/limelight/PosterContentProvider.java)**

`openFile`, `openBoxArtFile`, `delete`, `getType`, `insert`, `onCreate`, `query`, `update`, `createBoxArtUri`.

**[StreamView.java](../app/src/main/java/com/limelight/ui/StreamView.java)**

`initializeSurface`, `getHolder`, `getTextureView`, `setDesiredAspectRatio`, `setInputCallbacks`, `StreamView`, `onCheckIsTextEditor`, `onCreateInputConnection`, `commitText`, `deleteSurroundingText`, `deleteSurroundingTextInCodePoints`, `sendKeyPress`, `onMeasure`, `onKeyPreIme`, `handleKeyUp`, `handleKeyDown`, `handleTextInput`.

**[AdapterFragment.java](../app/src/main/java/com/limelight/ui/AdapterFragment.java)**

`onAttach`, `onCreateView`, `onActivityCreated`.

**[InputCaptureManager.java](../app/src/main/java/com/limelight/binding/input/capture/InputCaptureManager.java)**

`getInputCaptureProvider`.

**[InputCaptureProvider.java](../app/src/main/java/com/limelight/binding/input/capture/InputCaptureProvider.java)**

`enableCapture`, `disableCapture`, `destroy`, `isCapturingEnabled`, `isCapturingActive`, `showCursor`, `hideCursor`, `eventHasRelativeMouseAxes`, `getRelativeAxisX`, `getRelativeAxisY`, `onWindowFocusChanged`.

**[AndroidNativePointerCaptureProvider.java](../app/src/main/java/com/limelight/binding/input/capture/AndroidNativePointerCaptureProvider.java)**

`AndroidNativePointerCaptureProvider`, `isCaptureProviderSupported`, `hasCaptureCompatibleInputDevice`, `showCursor`, `hideCursor`, `onWindowFocusChanged`, `run`, `eventHasRelativeMouseAxes`, `getRelativeAxisX`, `getRelativeAxisY`, `onInputDeviceAdded`, `onInputDeviceRemoved`, `onInputDeviceChanged`.

**[AndroidPointerIconCaptureProvider.java](../app/src/main/java/com/limelight/binding/input/capture/AndroidPointerIconCaptureProvider.java)**

`AndroidPointerIconCaptureProvider`, `isCaptureProviderSupported`, `hideCursor`, `showCursor`.

**[ShieldCaptureProvider.java](../app/src/main/java/com/limelight/binding/input/capture/ShieldCaptureProvider.java)**

`ShieldCaptureProvider`, `isCaptureProviderSupported`, `setCursorVisibility`, `hideCursor`, `showCursor`, `eventHasRelativeMouseAxes`, `getRelativeAxisX`, `getRelativeAxisY`.

**[NullCaptureProvider.java](../app/src/main/java/com/limelight/binding/input/capture/NullCaptureProvider.java)**

Inherits the base capture behavior; no declared functions.

**[EvdevCaptureProviderShim.java](../app/src/main/java/com/limelight/binding/input/evdev/EvdevCaptureProviderShim.java)**

`isCaptureProviderSupported`, `createEvdevCaptureProvider`.

**[EvdevCaptureProvider.java](../app/src/root/java/com.limelight/binding/input/evdev/EvdevCaptureProvider.java)**

`run`, `EvdevCaptureProvider`, `reportDeviceNotRooted`, `runInNetworkSafeContextSynchronously`, `showCursor`, `hideCursor`, `enableCapture`, `destroy`.

**[MdnsDiscoveryAgent.java](../app/src/main/java/com/limelight/nvstream/mdns/MdnsDiscoveryAgent.java)**

`MdnsDiscoveryAgent`, `startDiscovery`, `stopDiscovery`, `reportNewComputer`, `getComputerSet`, `getLocalAddress`, `getLinkLocalAddress`, `getBestIpv6Address`.

**[NsdManagerDiscoveryAgent.java](../app/src/main/java/com/limelight/nvstream/mdns/NsdManagerDiscoveryAgent.java)**

`createDiscoveryListener`, `onStartDiscoveryFailed`, `onStopDiscoveryFailed`, `onDiscoveryStarted`, `onDiscoveryStopped`, `onServiceFound`, `onServiceInfoCallbackRegistrationFailed`, `onServiceUpdated`, `onServiceLost`, `onServiceInfoCallbackUnregistered`, `NsdManagerDiscoveryAgent`, `startDiscovery`, `stopDiscovery`, `getV4Addrs`, `getV6Addrs`.

**[JmDNSDiscoveryAgent.java](../app/src/main/java/com/limelight/nvstream/mdns/JmDNSDiscoveryAgent.java)**

`serviceAdded`, `serviceRemoved`, `serviceResolved`, `useInetAddress`, `newNetworkTopologyDiscovery`, `referenceResolver`, `dereferenceResolver`, `JmDNSDiscoveryAgent`, `handleResolvedServiceInfo`, `handleServiceInfo`, `startDiscovery`, `run`, `stopDiscovery`.

**[NvConnection.java](../app/src/main/java/com/limelight/nvstream/NvConnection.java)**

`NvConnection`, `generateRiAesKey`, `generateRiKeyId`, `stop`, `resolveServerAddress`, `detectServerConnectionType`, `negotiateVideoFormats`, `startApp`, `quitAndLaunch`, `launchNotRunningApp`, `launchApp`, `start`, `run`, `getHostDetails`, `canSendInput`, `refreshHostDetails`, `getClipboard`, `disconnectRemoteSession`, `setBitrate`, `sendClipboard`, `sendServerCommand`, `sendMouseMove`, `sendMousePosition`, `sendMouseMoveAsMousePosition`, `sendMouseButtonDown`, `sendMouseButtonUp`, `sendControllerInput`, `sendKeyboardInput`, `sendMouseScroll`, `sendMouseHScroll`, `sendMouseHighResScroll`, `sendMouseHighResHScroll`, `sendTouchEvent`, `sendPenEvent`, `sendControllerArrivalEvent`, `sendControllerTouchEvent`, `sendControllerMotionEvent`, `sendControllerBatteryEvent`, `sendUtf8Text`, `findExternalAddressForMdns`.

**[MoonBridge.java](../app/src/main/java/com/limelight/nvstream/jni/MoonBridge.java)**

`CAPABILITY_SLICES_PER_FRAME`, `AudioConfiguration`, `getSurroundAudioInfo`, `equals`, `hashCode`, `toInt`, `bridgeDrSetup`, `bridgeDrStart`, `bridgeDrStop`, `bridgeDrCleanup`, `bridgeDrSubmitDecodeUnit`, `bridgeArInit`, `bridgeArStart`, `bridgeArStop`, `bridgeArCleanup`, `bridgeArPlaySample`, `bridgeClStageStarting`, `bridgeClStageComplete`, `bridgeClStageFailed`, `bridgeClConnectionStarted`, `bridgeClConnectionTerminated`, `bridgeClRumble`, `bridgeClConnectionStatusUpdate`, `bridgeClSetHdrMode`, `bridgeClRumbleTriggers`, `bridgeClSetMotionEventState`, `bridgeClSetControllerLED`, `bridgeClSetAdaptiveTriggers`, `setupBridge`, `cleanupBridge`, `startConnection`, `stopConnection`, `interruptConnection`, `sendMouseMove`, `sendMousePosition`, `sendMouseMoveAsMousePosition`, `sendMouseButton`, `sendMultiControllerInput`, `sendTouchEvent`, `sendPenEvent`, `sendControllerArrivalEvent`, `sendControllerTouchEvent`, `sendControllerMotionEvent`, `sendControllerBatteryEvent`, `sendKeyboardInput`, `sendMouseHighResScroll`, `sendMouseHighResHScroll`, `sendUtf8Text`, `sendServerCommand`, `getStageName`, `findExternalAddressIP4`, `getPendingAudioDuration`, `getPendingVideoFrames`, `testClientConnectivity`, `getPortFlagsFromStage`, `getPortFlagsFromTerminationErrorCode`, `stringifyPortFlags`, `getEstimatedRttInfo`, `getLaunchUrlQueryParameters`, `guessControllerType`, `guessControllerHasPaddles`, `guessControllerHasShareButton`, `init`.

**[UiHelper.java](../app/src/main/java/com/limelight/utils/UiHelper.java)**

`setGameModeStatus`, `notifyStreamConnecting`, `notifyStreamConnected`, `notifyStreamEnteringPiP`, `notifyStreamExitingPiP`, `notifyStreamEnded`, `setLocale`, `notifyNewRootView`, `showDecoderCrashDialog`, `run`, `displayQuitConfirmationDialog`, `onClick`, `displayDeletePcConfirmationDialog`.

**[ShortcutHelper.java](../app/src/main/java/com/limelight/utils/ShortcutHelper.java)**

`ShortcutHelper`, `reapShortcutsForDynamicAdd`, `getAllShortcuts`, `getInfoForId`, `isExistingDynamicShortcut`, `reportComputerShortcutUsed`, `reportGameLaunched`, `createAppViewShortcut`, `createAppViewShortcutForOnlineHost`, `getShortcutIdForGame`, `createPinnedGameShortcut`, `disableComputerShortcut`, `disableAppShortcut`, `enableAppShortcut`.

**[TvChannelHelper.java](../app/src/main/java/com/limelight/utils/TvChannelHelper.java)**

`TvChannelHelper`, `updateChannelIcon`, `drawableToBitmap`, `getChannelId`, `getProgramId`, `toValueString`, `toUriString`, `isAndroidTV`, `setChannelId`, `setType`, `setTitle`, `setPosterArtAspectRatio`, `setIntent`, `setIntentUri`, `setInternalProviderId`, `setPosterArtUri`, `setWeight`, `toContentValues`, `setDisplayName`, `setAppLinkIntent`, `requestChannelOnHomeScreen`, `createTvChannel`, `addGameToChannel`, `deleteChannel`, `deleteProgram`.

**[SpinnerDialog.java](../app/src/main/java/com/limelight/utils/SpinnerDialog.java)**

`SpinnerDialog`, `displayDialog`, `closeDialogs`, `dismiss`, `setMessage`, `run`, `onCancel`.

**[Dialog.java](../app/src/main/java/com/limelight/utils/Dialog.java)**

`Dialog`, `closeDialogs`, `displayDialog`, `run`, `onClick`, `onShow`.

**[CrashCapture.java](../app/src/main/java/com/limelight/utils/CrashCapture.java)**

`CrashCapture`, `installHandler`, `start`, `collectExitReason`, `showPendingReport`, `lastReport`, `onActivityResumed`, `onActivityPaused`, `onActivityCreated`, `onActivityStarted`, `onActivityStopped`, `onActivitySaveInstanceState`, `onActivityDestroyed`.

**[HelpLauncher.java](../app/src/main/java/com/limelight/utils/HelpLauncher.java)**

`launchUrl`, `launchSetupGuide`, `launchTroubleshooting`, `launchGameStreamEolFaq`.

**[ProblemReport.java](../app/src/main/java/com/limelight/utils/ProblemReport.java)**

`setDecoderDiagnostics`, `show`, `share`.

**[PyroWaveBandwidthTest.java](../app/src/main/java/com/limelight/utils/PyroWaveBandwidthTest.java)**

`PyroWaveBandwidthTest`, `show`, `start`, `measurement`, `cancel`.

**[ServerHelper.java](../app/src/main/java/com/limelight/utils/ServerHelper.java)**

`getCurrentAddressFromComputer`, `createPcShortcutIntent`, `createAppShortcutIntent`, `createStartIntent`, `doStart`, `doQuit`, `run`.

**[FrontendExporter.java](../app/src/main/java/com/limelight/utils/FrontendExporter.java)**

`FrontendExporter`, `loadApps`, `export`, `entryBaseName`, `belongsToThisHost`, `registerEsDeSystem`, `merge`, `mergeXml`, `copyCover`, `writeText`, `treeRoot`, `directory`, `create`, `listChildren`.

**[GlPreferences.java](../app/src/main/java/com/limelight/preferences/GlPreferences.java)**

`GlPreferences`, `readPreferences`, `writePreferences`.

**[AndroidAudioRenderer.java](../app/src/main/java/com/limelight/binding/audio/AndroidAudioRenderer.java)**

`AndroidAudioRenderer`, `createAudioTrack`, `setup`, `playDecodedAudio`, `start`, `stop`, `cleanup`.

**[PyroWaveDecoderRenderer.java](../app/src/main/java/com/limelight/binding/video/PyroWaveDecoderRenderer.java)**

`loadLibrary`, `isAvailable`, `getReadiness`, `getReadinessSummary`, `setup`, `getFormat`, `submitFrame`, `pollRenderedFrames`, `getLastGpuDecodeUs`, `getPresentMode`, `getLastRecordLossPercent`, `setHdrMode`, `cleanup`, `nativeGetReadiness`, `nativeCreate`, `nativeSubmitFrame`, `nativePollRenderedFrames`, `nativeGetLastGpuDecodeUs`, `nativeGetPresentMode`, `nativeGetLastRecordLossPercent`, `nativeSetHdrMode`, `nativeDestroy`.

**[DecoderPerformanceHints.java](../app/src/main/java/com/limelight/binding/video/DecoderPerformanceHints.java)**

`DecoderPerformanceHints`, `reportWorkDuration`, `close`.

**[WindowFocusActionQueue.java](../app/src/main/java/com/limelight/WindowFocusActionQueue.java)**

`add`, `runPending`, `clear`.

### Additional resource boundaries

- `ControllerHandler`: constructor, `stop`, `destroy`, `disableSensors`, `enableSensors`, device-added/removed/changed callbacks and per-controller context destruction/sensor migration. Checked stream stop versus final activity destruction, handler-thread shutdown, vibrator cancellation and unregistering input/sensor listeners. Input remapping algorithms are outside this audit.
- `MediaCodecDecoderRenderer`: constructor/render target, `setup`, decoder initialization/configuration, `start`, renderer/choreographer startup, foreground/background notifications, `prepareForStop`, `stop`, `cleanup`, HDR/recovery coordination and latency-writer closure. Native session serialization and driver teardown remain essential; driver stalls cannot be established by JVM tests.
- Root `EvdevReader`: `readAll`, `read`; `EvdevEvent` constructor/constants and `EvdevListener` callbacks. The shutdown audit traces socket-read exit into `EvdevCaptureProvider.destroy`; keyboard dispatch is covered by L12. Packet format/key mapping behavior is outside the lifecycle fixes.

### JNI/C function inventory

- `callbacks.c`: `DetachThread`, `JniEnvKeyInit`, `GetThreadEnv`, `MoonBridge_init` JNI export, `BridgeDrSetup`, `BridgeDrStart`, `BridgeDrStop`, `BridgeDrCleanup`, `BridgeDrSubmitDecodeUnit`, `BridgeArInit`, `BridgeArStart`, `BridgeArStop`, `BridgeArCleanup`, `BridgeArDecodeAndPlaySample`, `BridgeClStageStarting`, `BridgeClStageComplete`, `BridgeClStageFailed`, `BridgeClConnectionStarted`, `BridgeClConnectionTerminated`, `BridgeClRumble`, `BridgeClConnectionStatusUpdate`, `BridgeClSetHdrMode`, `BridgeClRumbleTriggers`, `BridgeClSetMotionEventState`, `BridgeClSetControllerLED`, `BridgeClSetAdaptiveTriggers`, `BridgeClLogMessage`, `hasFastAes`, `MoonBridge_startConnection` JNI export. Checked callback registration lifetime, attached-thread exit, decoder/audio ownership and exception exits. L10 addresses the two retained local-array sites.
- `simplejni.c`: JNI exports (common prefix `Java_com_limelight_nvstream_jni_MoonBridge_`) `sendMouseMove`, `sendMousePosition`, `sendMouseMoveAsMousePosition`, `sendMouseButton`, `sendMultiControllerInput`, `sendTouchEvent`, `sendPenEvent`, `sendControllerArrivalEvent`, `sendControllerTouchEvent`, `sendControllerMotionEvent`, `sendControllerBatteryEvent`, `sendKeyboardInput`, `sendMouseHighResScroll`, `sendMouseHighResHScroll`, `sendUtf8Text`, `stopConnection`, `interruptConnection`, `getStageName`, `findExternalAddressIP4`, `getPendingAudioDuration`, `getPendingVideoFrames`, `testClientConnectivity`, `getPortFlagsFromStage`, `getPortFlagsFromTerminationErrorCode`, `stringifyPortFlags`, `getEstimatedRttInfo`, `getLaunchUrlQueryParameters`, `guessControllerType`, `guessControllerHasPaddles`, `guessControllerHasShareButton`. Checked string/array releases and the process-global native connection boundary addressed by L07.
- `android_connection.c` and included common-C `Connection.c`: cancellation hook and `LiGetStageName`, `LiInterruptConnection`, `LiStopConnection`, `terminationCallbackThreadFunc`, `ClInternalConnectionTerminated`, `parseRtspPortNumberFromUrl`, `LiStartConnection`, `LiGetLaunchUrlQueryParameters`. Checked startup failure unwinding, stage ordering, detached termination callback and stop/cancel transitions; submodule unchanged.
- `evdev_reader.c`: `hasRelAxis`, `hasKey`, `outputEvdevData`, `pollThreadFunc`, `precheckDeviceForPolling`, `startPollForDevice`, `enumerateDevices`, `connectSocket`, `main`. Checked list-entry ownership, socket/process exit, thread lifetime and cross-thread grab state. L13/L14 repair thread-resource ownership and the data race.
- `pyrowave_renderer.cpp` lifecycle boundary: `Renderer::create`, failure-path ownership, `Renderer::~Renderer`/`destroy`, swapchain-resource destruction, and JNI exports `nativeGetReadiness`, `nativeCreate`, `nativeSubmitFrame`, `nativePollRenderedFrames`, `nativeGetLastGpuDecodeUs`, `nativeGetPresentMode`, `nativeGetLastRecordLossPercent`, `nativeSetHdrMode`, `nativeDestroy`. Checked native-window acquisition/release, partial initialization, device-idle teardown, Java synchronization and JNI array/reference lifetimes. Vulkan decode/render algorithms are outside this lifecycle audit.
