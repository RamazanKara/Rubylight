//! Application loopback via Windows' process-tree capture API (build 20348+).
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
    core::{Interface, implement},
};

// windows-implement 0.60 defaults to agile=true and provides IAgileObject
// and the free-threaded marshaler, as ActivateAudioInterfaceAsync requires.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Completion {
    // The API's callback owns this allocation even after our timeout, so a
    // late completion cannot read a dead stack activation parameter or PID.
    params: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    _process: crate::process::AudioProcess,
    sender: mpsc::SyncSender<()>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for Completion_Impl {
    fn ActivateCompleted(
        &self,
        _operation: windows::core::Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        // Both threads use the MTA. Retrieve the interface on the audio
        // thread instead of asking COM to marshal IAudioClient, whose proxy
        // is not registered on every supported Windows installation.
        let _ = self.sender.try_send(());
        Ok(())
    }
}

pub struct Activation {
    process: crate::process::AudioProcess,
    receiver: mpsc::Receiver<()>,
    operation: IActivateAudioInterfaceAsyncOperation,
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
        // PROPVARIANT's destructor frees VT_BLOB with CoTaskMemFree. This
        // blob borrows the callback's Box, which must remain its only owner.
        let params = ManuallyDrop::new(PROPVARIANT {
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
        });
        let callback: IActivateAudioInterfaceCompletionHandler = completion.into();
        // SAFETY: the callback owns the stable parameter allocation and PID handle;
        // the borrowed PROPVARIANT is read during activation, not PropVariantClear'd.
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*params),
                &callback,
            )
        }
        .context("starting Windows process audio activation")?;
        Ok(Self {
            process,
            receiver,
            operation,
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
            Ok(()) => {
                let mut result = windows::core::HRESULT(0);
                let mut interface = None;
                // SAFETY: the callback signalled completion; this thread
                // owns the operation in the same MTA as the callback, and
                // both out-pointers live through GetActivateResult.
                unsafe {
                    self.operation
                        .GetActivateResult(&mut result, &mut interface)
                }
                .context("retrieving Windows process audio activation result")?;
                result
                    .ok()
                    .context("Windows process audio activation failed")?;
                let client = interface
                    .context("Windows process audio activation returned no interface")?
                    .cast::<IAudioClient>()
                    .context("opening the process audio client")?;
                super::Loopback::from_process_client(output_channels, self.process.clone(), client)
                    .context("initializing Windows process audio capture")
                    .map(Some)
            }
            Err(mpsc::TryRecvError::Empty) if self.started.elapsed() < Duration::from_secs(3) => {
                Ok(None)
            }
            _ => anyhow::bail!("Windows process audio activation timed out or was interrupted"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use windows::{
        Win32::Foundation::E_ACCESSDENIED,
        core::{HRESULT, IUnknown, OutRef},
    };

    fn current_process() -> crate::process::AudioProcess {
        let pid = std::process::id();
        crate::process::AudioProcess::open(butterpollo_core::audio_scope::Identity {
            pid,
            started: crate::process::creation_time(pid),
        })
        .unwrap()
    }

    #[implement(IActivateAudioInterfaceAsyncOperation)]
    struct FailedOperation(Arc<AtomicUsize>);

    impl IActivateAudioInterfaceAsyncOperation_Impl for FailedOperation_Impl {
        fn GetActivateResult(
            &self,
            result: *mut HRESULT,
            _interface: OutRef<IUnknown>,
        ) -> windows::core::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            // SAFETY: the test calls this through GetActivateResult with a live HRESULT.
            unsafe {
                *result = E_ACCESSDENIED;
            }
            Ok(())
        }
    }

    #[test]
    fn completion_does_not_marshal_or_retrieve_the_audio_client() {
        let _com = crate::capture::ComGuard::new().unwrap();
        let process = current_process();
        let calls = Arc::new(AtomicUsize::new(0));
        let operation: IActivateAudioInterfaceAsyncOperation =
            FailedOperation(calls.clone()).into();
        let (sender, receiver) = mpsc::sync_channel(1);
        let callback: IActivateAudioInterfaceCompletionHandler = Completion {
            params: Box::default(),
            _process: process.clone(),
            sender,
        }
        .into();
        let mut pending = Activation {
            process,
            receiver,
            operation: operation.clone(),
            started: std::time::Instant::now(),
        };
        assert!(pending.poll(2).unwrap().is_none());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        // SAFETY: both COM objects remain alive throughout the callback.
        unsafe {
            callback.ActivateCompleted(&operation).unwrap();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        let error = pending.poll(2).err().unwrap();
        assert!(format!("{error:#}").contains("Windows process audio activation failed"));
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        pending.started -= Duration::from_secs(4);
        assert!(
            pending
                .poll(2)
                .err()
                .unwrap()
                .to_string()
                .contains("timed out")
        );
        drop(pending);
        // SAFETY: a late completion may arrive after cancellation; the callback still lives.
        unsafe {
            callback.ActivateCompleted(&operation).unwrap();
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    #[ignore = "requires Windows process loopback and the Windows Audio service"]
    fn native_process_audio_survives_repeated_activation_and_cancellation() -> Result<()> {
        let _com = crate::capture::ComGuard::new()?;
        let process = current_process();
        for _ in 0..50 {
            drop(Activation::new(process.clone())?);
            let pending = Activation::new(process.clone())?;
            let mut capture = loop {
                if let Some(capture) = pending.poll(2)? {
                    break capture;
                }
                std::thread::sleep(Duration::from_millis(5));
            };
            assert_eq!(capture.process_identity(), Some(process.identity));
            // This test process renders no audio, even if other applications do.
            if let Some(samples) = capture.read(240)? {
                assert!(samples.iter().all(|sample| sample.abs() < 0.0001));
            }
        }
        Ok(())
    }
}
