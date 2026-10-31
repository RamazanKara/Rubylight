//! What a device keeps from a stream: its chosen layout, resolution and scale.
use super::*;

impl Prepared {
    pub(super) fn active_layout(&self) -> Result<Vec<(String, bool)>> {
        #[cfg(test)]
        if let Some(snapshot) = &self.layout_snapshot {
            return Ok(snapshot.clone());
        }
        Ok(butterpollo_windows::display::monitors()?
            .into_iter()
            .map(|m| (m.display_name, m.primary))
            .collect())
    }
    /// Keep the layout the user chose for the stream's display (Windows' "Show
    /// only on 2", say) as the device's own, when its last stream disconnects.
    /// A paused lease's eventual destruction must never learn a local layout.
    pub(super) fn keep_chosen_layout(&self) {
        let (Some(uuid), Some(planned)) = (&self.layout_owner, &self.virtual_layout) else {
            return;
        };
        let Some(h) = self.host.upgrade() else {
            return;
        };
        let active = match self.active_layout() {
            Ok(monitors) => monitors,
            Err(error) => {
                tracing::debug!(%error, "the layout at the end of the stream could not be read");
                return;
            }
        };
        let Some(chosen) =
            butterpollo_core::display_policy::chosen_layout(planned, &self.output, &active)
        else {
            return;
        };
        let mut paired = h.paired.write().unwrap();
        let Some(client) = paired.clients.iter_mut().find(|c| c.uuid == *uuid) else {
            return;
        };
        // The layout the user set for the device, before or during the stream, is
        // theirs to change in the console; what the stream ended with (a game's
        // mode switch, Windows moving the primary) never replaces it (issue #40).
        if has_user_layout(client) {
            tracing::debug!(client = %client.name, planned = %planned, chosen, "the device has a display layout of its own; not learning the one the stream ended with");
            return;
        }
        tracing::info!(client = %client.name, planned = %planned, chosen, "keeping the display layout chosen during the stream for this device");
        client
            .extra
            .insert("virtual_display_layout".into(), serde_json::json!(chosen));
        client.extra.remove(SET_BY_USER);
        if let Some(inherited) = &self.inherited_layout {
            client
                .extra
                .insert(LEARNED_OVER.into(), serde_json::json!(inherited));
        }
        if let Err(error) = paired.save(&h.paired_path) {
            tracing::warn!(%error, "the device's display layout could not be saved");
        }
    }
    /// The resolution the stream sets on its own virtual display and the one
    /// Windows shows.
    pub(super) fn size_change(&self) -> Option<SizeChange> {
        #[cfg(test)]
        if let Some(snapshot) = self.size_snapshot {
            return Some(snapshot);
        }
        let set = self.display.as_ref()?.virtual_size()?;
        match applied_mode(&self.output) {
            Ok((width, height, _, _)) => Some((set, (width, height))),
            Err(error) => {
                tracing::debug!(%error, "the display resolution at the end of the stream could not be read");
                None
            }
        }
    }
    /// Keep the resolution the user chose for the stream's display in Windows
    /// (1440p under a 4K stream, say) for the device when its last stream
    /// disconnects, and put it back on resume and recovery from now on. The
    /// refresh is not kept: it stays the stream's, so VRR and the virtual
    /// refresh multipliers keep working.
    pub(super) fn keep_chosen_resolution(&mut self) {
        let Some(uuid) = self.resolution_owner.clone() else {
            return;
        };
        let Some(h) = self.host.upgrade() else {
            return;
        };
        let Some((set, (width, height))) = self.size_change() else {
            return;
        };
        if (width, height) == set || width == 0 || height == 0 {
            return;
        }
        if let Some(display) = &self.display {
            display.adopt_virtual_size(width, height);
        }
        self.recovery_dimensions = (width, height);
        self.mode_report.adopt((width, height));
        let mut paired = h.paired.write().unwrap();
        let Some(client) = paired.clients.iter_mut().find(|c| c.uuid == uuid) else {
            return;
        };
        let chosen = format!("{width}x{height}");
        tracing::info!(client = %client.name, resolution = %chosen, "keeping the display resolution chosen during the stream for this device");
        client
            .extra
            .insert(REMEMBERED_RESOLUTION.into(), serde_json::json!(chosen));
        if let Err(error) = paired.save(&h.paired_path) {
            tracing::warn!(%error, "the device's display resolution could not be saved");
        }
    }
    /// Puts the display's scale back as the stream left it, as resume does for the layout and
    /// the mode: the baseline a scale picked during the stream is compared with, else the
    /// configured scale. A paused display's scale changed at the PC is not the device's choice.
    pub(super) fn put_scale_back(&self) -> Result<()> {
        let scale = self.scale_baseline.map_or(self.recovery_scale, i64::from);
        if scale == 0 {
            return Ok(());
        }
        #[cfg(test)]
        self.scales_set.lock().unwrap().push(scale);
        #[cfg(not(test))]
        let applied = scale_display(
            &self.output,
            scale,
            self.recovery_dimensions,
            butterpollo_windows::display::virtual_scale,
        );
        #[cfg(test)]
        let applied: Result<()> = Ok(());
        match (applied, self.scale_baseline) {
            // The scale a device keeps is not worth failing the resume for.
            (Err(error), Some(_)) => {
                tracing::warn!(error = %format!("{error:#}"), scale, "Windows did not take the display scale back after the pause");
                Ok(())
            }
            (applied, _) => applied,
        }
    }
    /// The scale the stream's display shows.
    pub(super) fn scale_now(&self) -> Result<u32> {
        #[cfg(test)]
        if let Some(snapshot) = self.scale_snapshot {
            return Ok(snapshot);
        }
        butterpollo_windows::display::output_scale(&self.output)
    }
    /// Keep the scale the user chose for the stream's display in Windows for the
    /// device when its last stream disconnects, and put it back on recovery from
    /// now on. Only a scale Windows shows is kept, so one it refused never is.
    /// The refresh is not kept: the stream's rate always wins over a kept one.
    pub(super) fn keep_chosen_scale(&mut self) {
        let (Some(uuid), Some(baseline)) = (self.resolution_owner.clone(), self.scale_baseline)
        else {
            return;
        };
        let Some(h) = self.host.upgrade() else {
            return;
        };
        let chosen = match self.scale_now() {
            Ok(chosen) => chosen,
            Err(error) => {
                tracing::debug!(%error, "the display scale at the end of the stream could not be read");
                return;
            }
        };
        if chosen == baseline || chosen == 0 {
            return;
        }
        self.scale_baseline = Some(chosen);
        self.recovery_scale = i64::from(chosen);
        let mut paired = h.paired.write().unwrap();
        let Some(client) = paired.clients.iter_mut().find(|c| c.uuid == uuid) else {
            return;
        };
        tracing::info!(client = %client.name, scale = chosen, "keeping the display scale chosen during the stream for this device");
        client
            .extra
            .insert(REMEMBERED_SCALE.into(), serde_json::json!(chosen));
        if let Err(error) = paired.save(&h.paired_path) {
            tracing::warn!(%error, "the device's display scale could not be saved");
        }
    }
}
