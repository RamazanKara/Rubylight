//! Application loopback via Windows' process-tree capture API.
//! Activation is bounded; a failure never falls back to an endpoint capture.
use std::{
    mem::{ManuallyDrop, size_of},
    sync::mpsc,
    time::Duration,
};

use anyhow::{Context, Result};
use windows::{
    Win32::{
        Media::Audio::*,
        System::{
            Com::{BLOB, StructuredStorage::*},
            Variant::VT_BLOB,
        },
    },
    core::{AgileReference, Interface, implement},
};

// windows-implement 0.60 defaults to agile=true and provides IAgileObject
// and the free-threaded marshaler, as ActivateAudioInterfaceAsync requires.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completion {
    // The API's callback owns this allocation even after our timeout, so a
    // late completion cannot read a dead stack activation parameter or PID.
    params: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    _process: crate::process::AudioProcess,
    sender: mpsc::SyncSender<windows::core::Result<AgileReference<IAudioClient>>>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(
        &self,
        operation: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let activated = (|| {
            let operation = operation.ok()?;
            let mut result = windows::core::HRESULT(0);
            let mut interface = None;
            // SAFETY: the OS supplies the live operation; both out-pointers
            // remain valid, and the returned interface owns its COM reference.
            unsafe {
                operation.GetActivateResult(&mut result, &mut interface)?;
            }
            result.ok()?;
            let client = interface
                .ok_or_else(windows::core::Error::empty)?
                .cast::<IAudioClient>()?;
            AgileReference::new(&client)
        })();
        // The receiver may have timed out. Dropping a late result releases it.
        let _ = self.sender.try_send(activated);
        Ok(())
    }
}

pub struct Activation {
    process: crate::process::AudioProcess,
    receiver: mpsc::Receiver<windows::core::Result<AgileReference<IAudioClient>>>,
    _operation: IActivateAudioInterfaceAsyncOperation,
    started: std::time::Instant,
}
impl Activation {
    pub fn new(process: crate::process::AudioProcess) -> Result<Self> {
        anyhow::ensure!(process.alive(), "game process is no longer running");
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut completion = Completion {
            params: Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
                ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
                Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                    ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                        TargetProcessId: process.identity.pid,
                        ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            }),
            _process: process.clone(),
            sender,
        };
        let params = PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: (&mut *completion.params
                                as *mut AUDIOCLIENT_ACTIVATION_PARAMS)
                                .cast(),
                        },
                    },
                    ..Default::default()
                }),
            },
        };
        let callback: IActivateAudioInterfaceCompletionHandler = completion.into();
        // SAFETY: the callback owns the stable parameter allocation and PID handle;
        // the borrowed PROPVARIANT is read during activation, not PropVariantClear'd.
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&params),
                &callback,
            )
        }
        .context("Windows process audio activation failed")?;
        Ok(Self {
            process,
            receiver,
            _operation: operation,
            started: std::time::Instant::now(),
        })
    }
    pub fn identity(&self) -> butterpollo_core::audio_scope::Identity {
        self.process.identity
    }
    /// Poll without waiting so a slow activation keeps sending silence at the
    /// negotiated packet cadence. Dropping this also safely discards late results.
    pub fn poll(&self, output_channels: usize) -> Result<Option<super::Loopback>> {
        match self.receiver.try_recv() {
            Ok(result) => {
                let client = result
                    .context("Windows process audio capture is unavailable")?
                    .resolve()
                    .context("opening the process audio client")?;
                super::Loopback::from_process_client(output_channels, self.process.clone(), client)
                    .map(Some)
            }
            Err(mpsc::TryRecvError::Empty) if self.started.elapsed() < Duration::from_secs(3) => {
                Ok(None)
            }
            _ => anyhow::bail!("Windows process audio activation timed out or was interrupted"),
        }
    }
}
