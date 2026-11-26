/**
 * Pure-JDK JNI declarations for {@code nightjar-policy} and {@code nightjar-core} policy
 * controllers (`protocol/android/src/policy.rs`). Loads {@code nightjar_core} via
 * {@link NightjarJni#ensureLibraryLoaded()}.
 */
package com.limelight.nvstream.jni;

public final class NightjarPolicyJni {
    static {
        NightjarJni.ensureLibraryLoaded();
    }

    private NightjarPolicyJni() {
    }

    // AdaptiveBitrateController
    public static native int abrBuiltInFloorKbps(int width, int height, int fps, int codecOrdinal);
    public static native long abrCreate(int ceilingKbps, int currentKbps, int fps, long nowMs,
                                        int width, int height, int codecOrdinal);
    public static native void abrDestroy(long handle);
    public static native int abrSample(long handle, long nowMs, boolean recentVideo,
                                       boolean poorConnection, float lossPercent,
                                       float decodeMs, float jitterMs, float queueMs);
    public static native String abrLastReason(long handle);
    public static native void abrSetThermalLevel(long handle, int level);
    public static native void abrSetFrameRate(long handle, int fps);
    public static native void abrSetStreamSize(long handle, int width, int height);
    public static native boolean abrHasRoomBeforeQualityReduction(long handle);
    public static native void abrApplied(long handle, int requestedKbps, int appliedKbps, long nowMs);
    public static native void abrSuspend(long handle);
    public static native int abrCeilingKbps(long handle);
    public static native int abrFloorKbps(long handle);

    // ClientHintBitrateController
    public static native int[] clientHintRange(int initialKbps);
    public static native long clientHintCreate(int initialKbps, int limitKbps, long nowMs);
    public static native void clientHintDestroy(long handle);
    public static native void clientHintResumeFrom(long handle, int kbps);
    public static native int clientHintSample(long handle, long nowMs, boolean recentVideo,
                                              float lossPercent, float queueMs);
    public static native void clientHintApplied(long handle, int kbps, long nowMs);
    public static native void clientHintRefused(long handle);
    public static native void clientHintOverridden(long handle, int kbps, long nowMs);
    public static native int[] clientHintState(long handle);

    // PyroWaveBitrateFloor
    public static native int pyroFloorMinimumKbps(int width, int height, int fps);
    public static native int pyroFloorDefaultKbps(int width, int height, int fps, int linkMbps);
    public static native int pyroFloorStartKbps(int configured, boolean automatic, int width,
                                                int height, int fps, int linkMbps);
    public static native int[] pyroFloorLinkNotice(int width, int height, int fps, int linkMbps,
                                                   int configuredKbps);

    // PyroWaveBitrateController
    public static native long pyroAbrCreate(int ceilingKbps, int currentKbps, int fps, long nowMs);
    public static native void pyroAbrDestroy(long handle);
    public static native int pyroAbrSample(long handle, long nowMs, boolean recentVideo,
                                           boolean poorConnection, float lossPercent,
                                           float queueMs, float decodeMs, float jitterMs,
                                           int thermalLevel);
    public static native void pyroAbrApplied(long handle, int requestedKbps, int appliedKbps, long nowMs);
    public static native void pyroAbrSuspend(long handle);

    // AdaptiveQualityController
    public static native double qualityDecodeBudgetMs(int fps);
    public static native long qualityCreate(int modeOrdinal, int width, int height, int fps,
                                            int minShortEdge, int minFps, int alignWidth,
                                            int alignHeight, long nowMs);
    public static native void qualityDestroy(long handle);
    public static native void qualityResumeAt(long handle, int width, int height, int fps);
    public static native int[] qualityCeiling(long handle);
    public static native int[] qualityCurrent(long handle);
    public static native int[] qualityLevels(long handle);
    public static native int[] qualityResizeCeiling(long handle, int width, int height, int fps, long nowMs);
    public static native int[] qualitySample(long handle, long nowMs, boolean recentVideo,
                                             boolean poorConnection, float lossPercent,
                                             float decodeMs, float jitterMs, float queueMs,
                                             int thermalLevel, boolean bitrateHasRoom,
                                             boolean observeTail, boolean hasTiming,
                                             long timingGeneration, long timingWindowStartNs,
                                             long timingWindowEndNs, long timingFirstSampleNs,
                                             long timingLastSampleNs, int timingFps,
                                             int timingSamples, int timingLateFrames);
    public static native void qualityApplied(long handle, int width, int height, int fps, long nowMs);
    public static native void qualitySuspend(long handle);

    // FoundationAbr
    public static native long foundationAbrCreate(int initialKbps, int streamKbps, long nowMs);
    public static native void foundationAbrDestroy(long handle);
    public static native void foundationAbrAdd(long handle, int completed, int drops, int decoded);
    public static native void foundationAbrAddRate(long handle, double lossPercent, double fps);
    public static native String foundationAbrDue(long handle, long nowMs, double rttMs);
    public static native int foundationAbrAnswered(long handle, boolean hasAction, int newBitrate,
                                                   boolean bitrateApplied);
    public static native int foundationAbrCurrentKbps(long handle);

    // PyroWavePace + PaceSearch
    public static native int paceWireKbps(int imageKbps, int fps, int packetSize, int fec, int minParity);
    public static native int paceProbeKbps(int imageKbps, int fps, int packetSize, int fec, int minParity);
    public static native int paceFloorKbps(int probeKbps);
    public static native long paceExpectedPackets(int kbps, int packetSize);
    public static native long paceBurstGroupPackets(int paceKbps, int packetSize, boolean ipv6);
    public static native String paceSummarize(long expected, long sent, double senderMs, boolean[] arrived,
                                              int fps, long groupPackets);
    public static native boolean paceMeasured(String measurementJson);
    public static native boolean paceQualified(String measurementJson);
    public static native int paceJumboProbeSize(int configured);
    public static native boolean paceJumboPasses(String jumboJson, String controlJson);
    public static native long paceSearchCreate(int linkKbps, int floorKbps);
    public static native void paceSearchDestroy(long handle);
    public static native int paceSearchNextPace(long handle);
    public static native void paceSearchRecord(long handle, int paceKbps, String measurementJson);
    public static native String paceSearchOutcome(long handle);

    // Update
    public static native String semVerParse(String text);
    public static native int semVerCompare(String a, String b);
    public static native boolean semVerIsNewer(String candidate, String current);
    public static native String semVerNormalize(String text);
    public static native String releaseInfoParse(String json);
    public static native boolean releaseInfoIsUpdateFor(String releaseJson, String currentVersion);
    public static native String releaseInfoApkAsset(String releaseJson, boolean root);
    public static native String releaseInfoChecksumAsset(String releaseJson);
    public static native String checksumFileFind(String sums, String fileName);
    public static native String checksumFileHex(byte[] bytes);
    public static native String releaseNotesPlain(String markdown, int maxChars);
    public static native boolean updatePolicyIsCheckDue(boolean autoCheck, long lastCheckMs, long nowMs);
    public static native boolean updatePolicyShouldOffer(String releaseJson, String currentVersion,
                                                         String skippedVersion);
    public static native boolean updatePolicyIsManagedByStore(String installerPackage);
    public static native boolean updatePolicyIsSelfUpdateAvailable(String installerPackage,
                                                                   boolean githubDistribution);

    // Host polling & lifecycle flow policies (`nightjar_policy::hosts`)
    public static native long hostSleepTokenRefreshMs();
    public static native long hostPcPowerRefusalPollMs();
    public static native int hostPcPowerRefusalReads();
    public static native long hostUpdateWaitMs();
    public static native long hostUpdateMaxWaitMs();
    public static native long hostUpdatePollMs();

    public static native int sleepUnavailableReason(boolean sleepSupported, boolean sleepAllowed);

    public static native long pairingWatchCreate();
    public static native void pairingWatchDestroy(long handle);
    public static native void pairingWatchRefreshSoon(long handle);
    public static native void pairingWatchPause(long handle);
    public static native long pairingWatchBeginPoll(
            long handle, long nowMs, boolean hasAskedHosts);
    public static native int pairingWatchFinishPoll(
            long handle, long generation, String idsJson);

    public static native String stripBidiControls(String input);
    public static native boolean pairingRefusedStatus(int status);
    public static native boolean alreadyPaired(
            boolean isPaired, boolean hasLink, boolean hasPinnedCert);
    public static native int classifyPairingFailure(
            int stateOrdinal, boolean fromLink, boolean oneTime, boolean inGame);
    public static native int classifyPairingHttpRefusal(boolean fromLink, boolean oneTime);
    public static native int classifyInviteError(String msg);
    public static native boolean isTunnelAddress(String ip);

    // Recovery, Exit, ContinueHere, InputOnly, Roaming (`protocol/android/src/policy_recovery.rs`)
    public static final int INPUT_ONLY_SMALL_WIDTH = 640;
    public static final int INPUT_ONLY_SMALL_HEIGHT = 360;
    public static final int INPUT_ONLY_SMALL_FPS = 30;
    public static final int INPUT_ONLY_SMALL_BITRATE_KBPS = 500;
    public static final int INPUT_ONLY_MODE_NATIVE = 0;
    public static final int INPUT_ONLY_MODE_SMALL_STREAM = 1;
    public static final int INPUT_ONLY_MODE_OFF = 2;
    public static final int INPUT_ONLY_BLOCKER_NONE = 0;
    public static final int INPUT_ONLY_BLOCKER_PAIR = 1;
    public static final int INPUT_ONLY_BLOCKER_VIEW_PERMISSION = 2;
    public static final int INPUT_ONLY_BLOCKER_INPUT_PERMISSION = 3;
    public static final int RESUME_REFUSAL_NONE = 0;
    public static final int RESUME_REFUSAL_SESSION_CHANGED = 1;
    public static final int RESUME_REFUSAL_BROWSER_STREAM = 2;
    public static final int ROAMING_CAP_IGNORE = 0;
    public static final int ROAMING_CAP_ASK_NOT_METERED = 1;
    public static final int ROAMING_CAP_ASK_METERED = 2;
    public static final int ROAMING_CAP_RETURNED_ASK_NOT_METERED = 3;
    public static final int ROAMING_CAP_RETURNED_ASK_METERED = 4;
    public static final int ROAMING_ADDR_IGNORE = -1;
    public static final int ROAMING_ADDR_ASK_NOT_METERED = 0;
    public static final int ROAMING_ADDR_ASK_METERED = 1;

    public static native int recoveryMaxAttempts();
    public static native long recoveryWindowMs();
    public static native long recoveryTripResumeDelayMs();
    public static native int recoveryPeerTimeoutMs();
    public static native long[] recoveryNextDelay(int attempts, long startedAtMs, long nowMs, long connectedAtMs);
    public static native long[] recoveryResetAfterStableConnection(int attempts, long startedAtMs, long nowMs, long connectedAtMs);
    public static native long[] recoveryAfterUserTrip(int attempts, long startedAtMs, long leftAtMs, long nowMs);
    public static native long recoveryTripConnectedBefore(int attempts, long connectedForMs);
    public static native boolean recoveryRecoversTermination(int code, boolean videoStarted);
    public static native boolean recoveryTransientTermination(int code);
    public static native boolean recoveryTransportTermination(int code);
    public static native boolean recoveryTransientStartup(String stage, int portFlags, int error);
    public static native boolean recoveryReconnectsAtOnce(boolean cannotContinue, boolean metered, boolean mobileAllowed,
                                                          boolean autoReconnect, int networkGraceMs);
    public static native String recoveryGaveUpCause(String lastFailure, String unreachable);
    public static native String recoveryGaveUpText(String cause, String stopped);
    public static native int recoveryAutomaticBitrateStart(int encoderKbps, int resumeKbps);
    public static native int recoveryReconnectBitrateStart(boolean automatic, boolean automaticOwned, boolean manual, int kbps);
    public static native boolean recoveryVideoStarted(boolean decoderReceivedVideo, boolean chainVideoShown);
    public static native boolean recoveryIsAttempt(int attempts, boolean tripResume);
    public static native boolean recoveryIsResumeOnly(int attempts, boolean tripResume, boolean outputResumeOnly);
    public static native boolean recoveryHandOffInPlace(boolean taskRoot, boolean multiWindow, boolean pictureInPicture);
    public static native boolean recoveryCanAutoRecover(
            boolean autoReconnect, boolean displayedFailure, boolean connected, boolean foreground,
            boolean videoStarted, boolean finishingOrDestroyed, boolean handoffOrPending,
            boolean streamRole, boolean controlAction, boolean hasResumeTarget,
            int attempts, long startedAtMs, long nowMs, long connectedAtMs);
    public static native boolean recoveryCanRememberResume(
            boolean alreadyRemembered, boolean autoReconnect, boolean connected, boolean displayedFailure,
            boolean finishingOrDestroyed, boolean cancelledOnPause, boolean handoffInProgress,
            int recoveryAppId, boolean streamRole, boolean controlAction);

    public static native boolean continueHereAsksOnSelect(boolean offered, boolean plainStart, boolean sameGame);
    public static native boolean continueHereReadsMoveAfterEnd(boolean offered, boolean leftOnPurpose, boolean recoveryFollows);
    public static native boolean continueHereRecovers(boolean transientFailure, boolean moved);
    public static native boolean continueHereCameBack(boolean moved, boolean owns);
    public static native boolean continueHereJoinsAfterRead(boolean too, boolean sameGame, boolean pressFailed);
    public static native int continueHereResumeRefusal(boolean join, int code, String message);

    public static native boolean exitQuitsOnLeave(boolean quitOnLeaveChosen, int roleOrdinal, boolean connected, boolean secondScreen);
    public static native boolean exitTerminationNeedsAction(int errorCode, boolean portTestBlocked,
                                                              boolean networkGraceGaveUp, boolean recoveryExhausted);
    public static native boolean exitHostEndedStream(int errorCode);
    public static native boolean exitRefusedInHandshake(String stage);

    public static native int inputOnlyMode(boolean anyHostEnabled, boolean remoteSessionTiles, boolean hostListsInputOnly);
    public static native int inputOnlySmallStreamBlocker(boolean paired, long permission);
    public static native boolean inputOnlyKeepsRunningGame(boolean hostRunningGame, boolean thisGameRunning);
    public static native int inputOnlyEstimatedKbps();
    public static native int inputOnlyEstimatedMegabytesPerHour();

    public static native long roamingCreate(boolean hasStreamNetwork, long streamNetworkId);
    public static native void roamingDestroy(long handle);
    public static native int roamingOnCapabilities(long handle, long networkId, boolean notMetered);
    public static native boolean roamingRecordChange(long handle, long networkId, boolean metered, boolean returned, int change, long nowMs);
    public static native int roamingOnAddresses(long handle, long networkId, String addressesKey, boolean nat64, long nowMs);
    public static native boolean roamingRecordAddressChange(long handle, int change);
    public static native boolean roamingOnLost(long handle, long networkId);
    public static native void roamingStop(long handle);

    // Video Pacing, Vsync, RefreshSlots (#119) & DisplayFrameRatePolicy (#168) (`policy_video_pacing.rs`)
    public static final int PACING_SLAB_HOLD_STAMP = 0;
    public static final int PACING_SLAB_REFRESH_SLOTS = 1;
    public static final int PACING_SLAB_ARR_HALF_RATE = 2;
    public static final int PACING_SLAB_CONTENT_RATE_VOTE = 3;

    public static native long pacingSlabCreate(int kind);
    public static native void pacingSlabDestroy(int kind, long handle);
    public static native boolean holdStampStep(long handle, long releaseNs, long vsyncNs, long intervalNs,
                                               long deadlineNs, int refreshes, boolean everyFrame,
                                               boolean vsyncOk, long[] out);
    public static native long vsyncTiming(int op, long a, long b, long c, long d, int e, boolean f);
    public static native boolean vsyncHeldReportGrid(long vsyncNs, long intervalNs, long anchorNs,
                                                     int refreshes, long[] out);
    public static native int vsyncRefreshesPerFrame(double refreshHz, double fps, boolean vrr, boolean fractional);

    public static native boolean framePacingBool(int op, int mode, boolean fa, boolean fb, boolean fc,
                                                 long a, long b, long c, int fpsMilliHz);
    public static native long framePacingTimeNs(boolean next, long dueNs, long vsyncNs, int fpsMilliHz);
    public static native int framePacingIntOp(int op, int a, int b, int c, int d, float hz);
    public static native float framePacingLimitRefreshHz(float askedHz, float startHz, int fps,
                                                         boolean batterySaver, boolean thermalCritical, float peakHz);

    public static native long refreshSlotsOp(int op, long handle, long a, long b);
    public static native boolean refreshSlotsPlan(long handle, long releaseNs, long vsyncNs, long intervalNs,
                                                  long leadNs, long[] out);
    public static native boolean refreshSlotsEarliestClaimed(long handle, long releaseNs, long vsyncNs,
                                                             long intervalNs, long leadNs);

    public static native int displayIntGate(int op, int a, int b, int c, int d, int e, int f, boolean flag, float hz);
    public static native boolean displayBoolGate(int op, int sdk, boolean a, boolean b, boolean c,
                                                 boolean d, boolean e, boolean f);
    public static native float displayRateCalc(int op, float a, float b, float c, boolean mayReduce, float[] seamlessHz);
    public static native int displayWindowModeForVote(int sdk, float voteHz, int flags, int[] activeMeta, float activeHz,
                                                      float[] activeAlts, int[] modeIds, int[] modeWs, int[] modeHs,
                                                      float[] modeHzs);
    public static native float displayMaxRefreshRate(int activeW, int activeH, float activeHz, int[] modeWs, int[] modeHs,
                                                     float[] modeHzs, int[] altCounts, float[] allAlts,
                                                     int streamW, int streamH);
    public static native boolean arrHalfRateStep(long handle, int op, long nowMs, float rendered, float presented,
                                                 float displayHz, float streamHz);
    public static native float contentRateVoteStep(long handle, int op, long nowMs, int contentMilliHz);

    // Video Queue, PresentScheduler, DisplayQueueWatch (#7), DecoderRecoveryBackoff (`policy_video_queue.rs`)
    public static final int QUEUE_SLAB_RESIDENCY = 0;
    public static final int QUEUE_SLAB_DISPLAY_GATE = 1;
    public static final int QUEUE_SLAB_RECENT_LEVEL = 2;
    public static final int QUEUE_SLAB_DEBT = 3;
    public static final int QUEUE_SLAB_IN_FLIGHT = 4;
    public static final int QUEUE_SLAB_DISPLAY_WATCH = 5;
    public static final int QUEUE_SLAB_PRESENT_SCHEDULER = 6;
    public static final int QUEUE_SLAB_RECOVERY_BACKOFF = 7;

    public static native long queueSlabCreate(int kind);
    public static native void queueSlabDestroy(int kind, long handle);
    public static native long recoveryBackoffCreate(long[] delaysMs);

    public static native int decoderQueueDepth(int framesIn, int framesOut, int base);
    public static native int decoderOutputLimit(String decoderName, boolean advertisedLowLatency,
                                                boolean dropLateFrames, boolean recovered);
    public static native boolean decoderWatchesDisplayQueue(String decoderName);

    public static native long residencyOp(int op, long handle, long a, long b);
    public static native long displayGateOp(int op, long handle, long a, long b, long c,
                                            boolean fa, boolean fb);
    public static native boolean displayGateStillHeld(long handle, int minBuckets, double maxSlope);
    public static native long recentLevelOp(int op, long handle, long a, long b);
    public static native long debtOp(int op, long handle, long a, long b, long c, long d, int e, boolean flag);
    public static native String debtTakeEvent(long handle);
    public static native long inFlightOp(int op, long handle, long a, long b);

    public static native boolean displayQueueApplies(boolean settingOn, boolean latencyOrBalanced,
                                                     boolean scheduled, boolean renderCallbacks,
                                                     boolean vrr, int streamFrameRateMilliHz, long refreshNs);
    public static native long displayQueueOp(int op, long handle, long a, long b, boolean flag);
    public static native String displayQueueTakeEvent(long handle);

    public static native boolean presentSchedulerIsLate(long handle, long hostUs, long readyNs,
                                                        long contentIntervalNs);
    public static native boolean presentSchedulerDropLate(boolean late, boolean newerAvailable,
                                                          boolean previousDropped);
    public static native long presentSchedulerOp(int op, long handle, boolean flag);
    public static native String presentSchedulerOverlayLine(long handle);

    public static native long recoveryBackoffOp(int op, long handle, int framesOut);
    public static native int decoderWantsCatchUp(long residencyHandle, int mode, long enqueueNs, long nowNs,
                                                 int fpsMilliHz, int drained, int maxOutputDrain,
                                                 boolean scheduled, int framesIn, int framesOut, int depthBase);
    public static native int decoderRecoveryOp(int op, boolean hasVendorKeys, boolean hasDecoder,
                                               boolean alreadyHungWithKeys, int attempts,
                                               int framesOut, int framesOutAtLastRecovery,
                                               int framesOutAtHangReset, long uptimeMs,
                                               long uptimeAtLastRecoveryMs, long uptimeAtHangResetMs);

    // Video Codec, FullRangePolicy (#153), VideoRendererChoice, Sdr10Policy, HdrPolicy, HdrFormatPolicy
    public static final int DISPLAY_HDR_HDR10 = 1 << 0;
    public static final int DISPLAY_HDR_HLG = 1 << 1;
    public static final int DISPLAY_HDR_DOLBY_VISION = 1 << 2;
    public static final int DISPLAY_HDR_HDR10_PLUS = 1 << 3;

    public static final int TRANSFER_NONE = 0;
    public static final int TRANSFER_PQ = 1;
    public static final int TRANSFER_HLG = 2;
    public static final int TRANSFER_OTHER = 3;

    public static final int COLOR_RANGE_ABSENT = 0;
    public static final int COLOR_RANGE_FULL = 1;
    public static final int COLOR_RANGE_LIMITED = 2;
    public static final int COLOR_RANGE_UNKNOWN = 3;

    public static final int PYROWAVE_DROP_DEVICE_REASON = 1;
    public static final int PYROWAVE_DROP_MISSING_SWAPCHAIN = 2;
    public static final int PYROWAVE_DROP_HDR_FAILURE = 3;

    public static final int THERMAL_STATE_NORMAL = 0;
    public static final int THERMAL_STATE_WARM = 1;
    public static final int THERMAL_STATE_WARM_HOLDING = 2;
    public static final int THERMAL_STATE_HOT = 3;
    public static final int THERMAL_STATE_HOT_EASING = 4;

    public static final int SURFACE_OP_VULKAN = 0;
    public static final int SURFACE_OP_PYROWAVE = 1;
    public static final int SURFACE_OP_PYROWAVE_WINDOW = 2;

    public static final int TEXT_OP_OVERLAY_WORD = 0;
    public static final int TEXT_OP_BASE_LAYER_NAME = 1;
    public static final int TEXT_OP_NAME = 2;
    public static final int TEXT_OP_CODEC_NAMES = 3;

    public static native String fullRangeOutputFallbackReason(
            boolean fullRange, boolean hdr, boolean dolbyVision, boolean packed444, boolean pyroWave,
            int colorRangeCode, int rawColorRange, boolean hasColorAspects);
    public static native boolean fullRangeCanUseVulkanSdk(int sdkInt);

    public static native boolean videoRendererNeedsVulkanProbe(
            boolean vulkanPreferred, boolean pyroWave, int sdkInt, boolean packed444,
            boolean frameWarp);
    public static native int videoRendererChoose(
            boolean vulkanPreferred, boolean pyroWave, int sdkInt, boolean vulkanSupported,
            boolean packed444, boolean upscalingOn, boolean frameWarp, boolean hdrOrTenBit);
    public static native boolean videoRendererFrameWarpAsked(boolean frameWarp, boolean frameWarpListed);

    public static native int sdr10Refusal(
            boolean mediaStream, boolean enabled, boolean hdrGranted, boolean hevcMain10,
            boolean av1Main10, boolean pyroWaveChosen, boolean packed444);
    public static native boolean sdr10WorthSaying(int refusalOrdinal);
    public static native boolean sdr10DecoderChoiceMayChange(
            boolean enabled, boolean hdrRequested, boolean pyroWaveChosen, boolean packed444);
    public static native boolean sdr10Yuv444Wins(int commonFormats);
    public static native int sdr10WithoutTenBit(int offeredFormats);
    public static native int sdr10HostHdrStep(boolean tenBit, int hostHdr, int previous);
    public static native int sdr10Formats(int refusalOrdinal, boolean hevcMain10, boolean av1Main10);
    public static native int sdr10Status(boolean tenBit, int hostHdr);
    public static native String sdr10OverlayColor(boolean tenBit, boolean sdr10, int hostHdr);
    public static native boolean sdr10AssumeHdr(boolean tenBit, boolean sdr10);

    public static native int hdrDisplayRefusal(
            int sdk, boolean enabled, int displayMask, int anyModeMask, int hostHdrFormats, int surfaceOrdinal);
    public static native boolean hdrShouldForceHlgOnly(boolean debugHlgOnly, int displayMask);
    public static native boolean hdrHasHdr10(int displayMask);
    public static native boolean hdrHlgOnly(int displayMask);
    public static native boolean hdrReportsOutput(int displayMask, int streamTransferCode);
    public static native int hdrDecoderRefusal(boolean hevcMain10Hdr10, boolean av1Main10, boolean pyroWave);
    public static native int hdrMetadata(byte[] wire);
    public static native int hdrDrawnStatus(int statusOrdinal, boolean toneMapped);
    public static native boolean hdrHandsBackFromVulkan(boolean hdrReported, boolean toneMapped);
    public static native boolean hdrHandOverAfterVulkanStop(boolean hadRenderer, boolean stopFinished);
    public static native int hdrStatus(
            boolean asked, boolean tenBit, int hostHdr, int metadataOrdinal,
            int outputTransferCode, int streamTransferCode, boolean hlgOnlyScreen);

    public static native int hdrFormatHlgWindowOf(int call);
    public static native int hdrFormatPick(
            int hostFormats, int displayMask, int dolbyVisionDecoders, int videoFormats, int surfaceOrdinal);
    public static native int hdrFormatVulkanLimit(
            int hostFormats, int displayMask, int dolbyVisionDecoders, int videoFormats);
    public static native boolean hdrFormatNeedsHlgProbe(
            boolean pyroWave, int hostFormats, int displayMask, int surfaceOrdinal);
    public static native int hdrFormatSurface(int op, int hostFormats, int surfaceOrdinal, int windowOrdinal);
    public static native boolean hdrFormatPassesHdr10Plus(int displayMask, int transferCode);
    public static native boolean hdrFormatPyroWaveHlgRefusedByDisplay(
            int hostFormats, int displayMask, int windowOrdinal);
    public static native boolean hdrFormatPyroWaveHlgLost(
            int announcedFormat, int windowOrdinal, boolean windowKeptHlg, boolean framesStopped);
    public static native int hdrFormatPyroWaveLimit(
            int hostFormats, int displayMask, int dolbyVisionDecoders, int format);
    public static native boolean hdrFormatPyroWaveHlgNeedsHostUpdate(int hostFormats, int displayMask);
    public static native boolean hdrFormatHlgInstead(int hostFormats, int displayMask);
    public static native boolean hdrFormatDolbyVision(int format);
    public static native int hdrFormatCarriers(int format);
    public static native int hdrFormatVideoFormats(int format, int videoFormats, boolean automaticCodec);
    public static native boolean hdrFormatTransferIsHlg(int format);
    public static native String hdrFormatText(int op, int format, boolean dolbyVisionDecoding, boolean hdr10Plus);

    public static native int videoMaxInputSize(int bitrateKbps, int fps, int width, int height);
    public static native int videoGrownInputSize(long requiredBytes, int capacity, int previousRequest);
    public static native int videoHdrChange(
            boolean previousMode, boolean enabled, byte[] previousMetadata, byte[] metadata, int sdkInt);
    public static native boolean videoRestartRecreates(String decoderName, String tuningKeysLines);
    public static native boolean videoDecoderCarriesHdrMetadata(boolean vulkanPresents);
    public static native boolean videoCarriesHdrStaticInfo(byte[] carried, byte[] expected);
    public static native boolean videoYuv444FallbackIsHdr(int format, boolean sdr10);
    public static native boolean videoPyroWaveFullChromaDrop(boolean packed444, boolean yuv444);
    public static native int videoPyroWaveDropReason(
            boolean chosen, boolean fullChroma, int deviceReason, int hostCodecs,
            int formats, int choice, int hdrFailure);
    public static native int[] videoPyroWaveCandidateFormats(int formats);
    public static native String videoHostTenBitIssue(int deviceModes, int hostModes);
    public static native String videoHostTenBitDrop(
            boolean tenBitOffered, String dropAtLaunch, int negotiatedFormat, int hostModes);
    public static native String videoTenBitOutputIssue(boolean displayHdr10, boolean gpuTenBitOutput);
    public static native int videoCapabilitiesFor(
            int format, boolean sdr10, boolean hasRunner, byte runnerSlices, boolean runnerDirectSubmit,
            byte slices8, byte slices10, boolean directSubmit8, boolean rfiAvc, boolean rfiHevc,
            boolean rfiAv1, boolean rfiHevc10, boolean rfiAv110);
    public static native long videoApplyDolbyVisionRfi(
            int capabilities, boolean dolbyVisionCorrupts, boolean dolbyVisionAv1Corrupts, int hdrFormat);
    public static native long videoRecordFrameGap(int gap, long newDecoderDrops);
    public static native int videoThermalState(int level, boolean holdsBitrate, boolean easesLoad);

    /**
     * The profile kept for a screen (`protocol/android/src/display_profile.rs`): {@code {"rules":[{"profile":"3",
     * "display":{"id":"builtin","name":""}}],"screen":{"id":"builtin","name":""}}} answers {@code {"profile":"3"}},
     * or {@code {"profile":null}} when no profile is kept for the screen.
     */
    public static native String displayProfile(String request);
}
