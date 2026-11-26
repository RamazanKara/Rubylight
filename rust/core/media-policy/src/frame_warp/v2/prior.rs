//! What Frame Warp 2 keeps for a game's next stream ([`PriorV2`]) and how a saved prior
//! starts the fast fits warm.

use rubylight_protocol::frame_warp::MAX_GAIN;
use serde::{Deserialize, Serialize};

use super::super::FrameWarpHost;
use super::super::calibrate::{DEFAULT_LAG_US, Prior};

/// What a stream learned for a game: the calibrator's prior and the gyro's part.
/// Saved by the host (`frame_warp.json` schema 2); a version 1 entry reads with no gyro.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PriorV2 {
    #[serde(flatten)]
    pub base: Prior,
    /// Picture centre pixels per radian of gyro yaw and pitch, at the base's picture size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gyro_gain: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gyro_lag_us: Option<u64>,
    /// The gyro's yaw is the motion packets' z axis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gyro_yaw_z: Option<bool>,
    /// Unix seconds of the last save made while a model was locked (the store's).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confident_at: Option<u64>,
}

/// The gyro's part of a [`PriorV2`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::frame_warp) struct GyroPrior {
    gain: Option<[f32; 2]>,
    lag_us: Option<u64>,
    yaw_z: Option<bool>,
    picture: (u16, u16),
}

/// A saved prior in thumbnail pixels, for warm starts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Warm {
    pub(super) mouse: Option<([f64; 2], u64)>,
    pub(super) stick: Option<([f64; 2], u64, (f32, f32))>,
    pub(super) gyro: Option<([f64; 2], u64)>,
    pub(super) yaw_z: Option<bool>,
}

impl FrameWarpHost {
    /// What the game's saved prior (or what the calibrator locked since) says, in thumbnail
    /// pixels.
    pub(super) fn v2_warm(&self) -> Warm {
        let scale = self.v2_scale();
        let content = (self.content.2.max(1), self.content.3.max(1));
        let thumb = |g: [f32; 2], picture: (u16, u16)| {
            let ratio = [
                f64::from(content.0) / f64::from(picture.0.max(1)),
                f64::from(content.1) / f64::from(picture.1.max(1)),
            ];
            [
                f64::from(g[0]) * ratio[0] / scale[0],
                f64::from(g[1]) * ratio[1] / scale[1],
            ]
        };
        let mut warm = Warm::default();
        if let Some(prior) = self.calibrator.prior() {
            let picture = (prior.picture_width, prior.picture_height);
            warm.mouse = prior
                .mouse_gain
                .zip(prior.mouse_lag_us)
                .map(|(g, lag)| (thumb(g, picture), lag));
            warm.stick = prior.stick_gain.zip(prior.stick_lag_us).map(|(g, lag)| {
                let curve = (
                    prior.stick_deadzone.unwrap_or(0.15),
                    prior.stick_exponent.unwrap_or(1.0),
                );
                (thumb(g, picture), lag, curve)
            });
        }
        if let Some(gyro) = self.v2_prior {
            warm.gyro = gyro.gain.map(|g| {
                (
                    thumb(g, gyro.picture),
                    gyro.lag_us.unwrap_or(DEFAULT_LAG_US),
                )
            });
            warm.yaw_z = gyro.yaw_z;
        }
        warm
    }

    /// Starts from what an earlier stream learned for the game: the calibrator's prior
    /// ([`FrameWarpHost::set_prior`]) and the gyro's; the fast fits start warm from it.
    pub fn set_prior_v2(&mut self, prior: PriorV2) {
        self.set_prior(prior.base);
        self.v2_prior = Some(GyroPrior {
            gain: prior
                .gyro_gain
                .filter(|g| g.iter().all(|v| v.is_finite() && v.abs() <= MAX_GAIN)),
            lag_us: prior.gyro_lag_us,
            yaw_z: prior.gyro_yaw_z,
            picture: (prior.base.picture_width, prior.base.picture_height),
        });
        let warm = self.v2_warm();
        if let Some(v2) = self.v2.as_mut() {
            v2.warm(&warm);
        }
    }

    /// What to remember for this game: [`FrameWarpHost::prior`], with what the fast fits locked
    /// where the calibrator has nothing, and the gyro's gain and yaw axis.
    pub fn prior_v2(&self) -> Option<PriorV2> {
        let content = (self.content.2.max(1), self.content.3.max(1));
        let mut base = self.prior();
        let mut gyro = self.v2_prior.unwrap_or_default();
        if let Some(v2) = self.v2.as_deref() {
            let scale = v2.scale;
            let picture = |g: [f64; 2]| [(g[0] * scale[0]) as f32, (g[1] * scale[1]) as f32];
            let [mouse, stick, gyro_fit] = v2.locked_fits();
            if mouse.is_some() || stick.is_some() {
                let prior = base.get_or_insert(Prior {
                    picture_width: content.0,
                    picture_height: content.1,
                    ..Prior::default()
                });
                let same = (prior.picture_width, prior.picture_height) == content;
                if let Some(fit) = mouse.filter(|_| same && prior.mouse_gain.is_none()) {
                    prior.mouse_gain = Some(picture(fit.gain));
                    prior.mouse_lag_us = Some(fit.lag_us);
                }
                if let Some(fit) = stick.filter(|_| same && prior.stick_gain.is_none()) {
                    prior.stick_gain = Some(picture(fit.gain));
                    prior.stick_lag_us = Some(fit.lag_us);
                    prior.stick_deadzone = Some(fit.deadzone);
                    prior.stick_exponent = Some(fit.exponent);
                }
            }
            if let Some(fit) = gyro_fit {
                gyro = GyroPrior {
                    gain: Some(picture(fit.gain)),
                    lag_us: Some(fit.lag_us),
                    yaw_z: v2.yaw.decided().then(|| v2.yaw.yaw_z()),
                    picture: content,
                };
            }
        }
        if base.is_none() && gyro.gain.is_none() {
            return None;
        }
        let base = base.unwrap_or(Prior {
            picture_width: gyro.picture.0,
            picture_height: gyro.picture.1,
            ..Prior::default()
        });
        // The gyro's gain in the base's picture pixels.
        let ratio = [
            f64::from(base.picture_width.max(1)) / f64::from(gyro.picture.0.max(1)),
            f64::from(base.picture_height.max(1)) / f64::from(gyro.picture.1.max(1)),
        ];
        Some(PriorV2 {
            base,
            gyro_gain: gyro.gain.map(|g| {
                [
                    (f64::from(g[0]) * ratio[0]) as f32,
                    (f64::from(g[1]) * ratio[1]) as f32,
                ]
            }),
            gyro_lag_us: gyro.lag_us,
            gyro_yaw_z: gyro.yaw_z,
            confident_at: None,
        })
    }
}
