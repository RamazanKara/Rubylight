//! The workers' answers, applied on the UI thread: `Hosts::drain` and what each answer changes in the list.

use super::*;

impl Hosts {
    /// Applies queued events. Returns those the UI reacts to (pairing, quit) after the list
    /// has taken them in, and whether the stored hosts changed.
    pub fn drain(&mut self, ctx: &egui::Context) -> (Vec<Event>, bool) {
        let mut out = Vec::new();
        let mut changed = false;
        while let Ok(event) = self.events.try_recv() {
            match event {
                Event::Info {
                    id,
                    seq,
                    address,
                    result,
                } => {
                    // An older answer than one already taken in describes a PC that has moved on.
                    match self.get_mut(id) {
                        Some(host) if seq < host.info_seq => continue,
                        Some(host) => host.info_seq = seq,
                        None => {}
                    }
                    changed |= self.apply_info(id, address, result);
                    self.follow_native(id);
                    self.follow_classic(id);
                    self.check_foundation_probe(id);
                    self.follow_internet_play(id);
                }
                Event::InternetPlay { id, unique_id, result } => {
                    let Some(host) = self.get_mut(id) else { continue };
                    match result {
                        // Another PC answers at this address by now: the reply is not its.
                        Ok(text) if host.record.unique_id == unique_id => {
                            if host.record.internet_play.as_deref() != Some(text.as_str()) {
                                host.record.internet_play = Some(text);
                                crate::logfile::add_secrets(&host.record);
                                changed = true;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("nightjar: the PC's internet play details could not be read: {e}"),
                    }
                }
                Event::FoundationProbe { id, unique_id, found } => {
                    if let Some(host) = self.get_mut(id) {
                        host.probing_foundation = false;
                        // Another PC answers at this address by now: the answer is not its.
                        if host.record.unique_id == unique_id {
                            host.foundation_probe = found;
                        }
                    }
                }
                Event::Discovered {
                    name,
                    addresses,
                    port,
                } => changed |= self.apply_discovery(name, addresses, port),
                Event::Remote { id, result } => match result {
                    Ok(Some(address)) => changed |= self.apply_remote_address(id, address),
                    Ok(None) => eprintln!("nightjar: STUN: no answer, the PC has no remote address yet"),
                    Err(e) => eprintln!("nightjar: STUN failed: {e}"),
                },
                Event::Paired { id, request, result } => {
                    if request != self.pairing_generation {
                        continue;
                    }
                    self.pairing = None;
                    if let (Ok(pem), Some(host)) = (&result, self.get_mut(id)) {
                        host.record.server_certificate = Some(pem.clone());
                        host.apps = Apps::NotLoaded;
                        changed = true;
                        self.poll_one(id);
                    }
                    out.push(Event::Paired { id, request, result });
                }
                Event::Apps { id, result } => {
                    let mut save = false;
                    if let Some(host) = self.get_mut(id) {
                        host.apps = match result {
                            Ok(mut apps) => {
                                apps.sort_by_key(|a| a.host_index.unwrap_or(u32::MAX));
                                // Home shows a paired PC's app list where it has no native library,
                                // and keeps it for when the PC is away.
                                if host.record.server_certificate.is_some() && !host.native_api() {
                                    save = host.set_classic_apps(Some(crate::config::ClassicApps::of(&apps)));
                                } else if host.native_api() {
                                    save = host.set_classic_apps(None);
                                }
                                Apps::Loaded(apps)
                            }
                            Err(e) => Apps::Failed(e),
                        };
                    }
                    if save && !self.classic_saves.contains(&id) {
                        self.classic_saves.push(id);
                    }
                }
                Event::ArtAway { id, app, version } => {
                    if let Some(host) = self.get_mut(id).filter(|h| h.art_versions.get(&app) == Some(&version)) {
                        // As a cover that was never asked for: the pages ask when the PC is up.
                        if matches!(host.art.get(&app), Some(Art::Loading)) {
                            host.art.remove(&app);
                        }
                    }
                }
                Event::ArtSharper { id, app, version, image, sized } => {
                    let frame = self.art_frame;
                    let Some(host) = self.list.iter_mut().find(|h| h.id == id).filter(|h| h.art_versions.get(&app) == Some(&version)) else { continue };
                    // Only the last read asked for lands: `art_sized` names the scale it was asked
                    // for, and none means the cover shows all of its file already. An older read
                    // landing late (behind a newer scale's, or after the cover was loaded again)
                    // would make it blurrier and stay so.
                    if let Some(sized) = sized {
                        if host.art_sized.get(&app).is_none_or(|&asked| sized < asked) {
                            continue;
                        }
                    }
                    // Still the cover it was loaded again for: one given up or replaced since is not.
                    let Some(Art::Ready(texture)) = host.art.get_mut(&app) else { continue };
                    *texture = ctx.load_texture(format!("art-{id}-{app}"), image, egui::TextureOptions::LINEAR);
                    match sized {
                        Some(sized) => {
                            host.art_sized.insert(app, sized);
                        }
                        None => {
                            host.art_sized.remove(&app);
                        }
                    }
                    self.art_used.insert(texture.id(), frame);
                    self.art_landed = true;
                }
                Event::Art { id, app, version, image, ambient, sized } => {
                    let frame = self.art_frame;
                    let mut landed = None;
                    if let Some(host) = self.get_mut(id).filter(|h| h.art_versions.get(&app) == Some(&version)) {
                        match ambient {
                            Some(ambient) => {
                                host.ambient_glow.insert(app, crate::ambient::glow_of(&ambient));
                                host.ambient.insert(app, ctx.load_texture(format!("ambient-{id}-{app}"), ambient, egui::TextureOptions::LINEAR));
                            }
                            None => {
                                host.ambient_glow.remove(&app);
                                host.ambient.remove(&app);
                            }
                        }
                        let art = match image {
                            Some(image) => {
                                let texture = ctx.load_texture(format!("art-{id}-{app}"), image, egui::TextureOptions::LINEAR);
                                landed = Some(texture.id());
                                Art::Ready(texture)
                            }
                            None => Art::Missing,
                        };
                        host.art.insert(app, art);
                        host.art_sized.remove(&app);
                        if let Some(sized) = sized.filter(|_| landed.is_some()) {
                            host.art_sized.insert(app, sized);
                        }
                    }
                    // It was asked for because it is on screen: not the first to go for room.
                    if let Some(texture) = landed {
                        self.art_used.insert(texture, frame);
                        self.art_landed = true;
                    }
                }
                Event::Native { id, epoch, seq, result } => {
                    let mut reload_apps = false;
                    let mut save = false;
                    // A read that started before an unpair says nothing about the PC now.
                    if let Some(host) = self.get_mut(id).filter(|h| h.native_epoch == epoch && h.record.server_certificate.is_some()) {
                        host.native.loading = false;
                        let newest = seq > host.action_seq;
                        match result {
                            Ok(read) => {
                                let read = *read;
                                host.native.error = None;
                                host.native.pairing_requests = read.pairing_requests;
                                if let Some(capabilities) = read.capabilities {
                                    host.native.update = read.update;
                                    if let Some(info) = host.info.as_mut() {
                                        merge_native(info, Some(&capabilities));
                                    }
                                    host.native.capabilities = Some(capabilities);
                                }
                                if let Some((macs, broadcasts)) = read.wake {
                                    if host.record.wake_macs != macs || host.record.wake_broadcasts != broadcasts {
                                        host.record.wake_macs = macs;
                                        host.record.wake_broadcasts = broadcasts;
                                        changed = true;
                                    }
                                }
                                if let Some(NativeLibrary { library, last_played, collections }) = read.library {
                                    // The classic list a launch goes through follows the library.
                                    reload_apps = !host.native.library_live
                                        || host.native.library.as_ref().is_none_or(|old| old.revision != library.revision);
                                    save = host.native.library.as_ref() != Some(&library)
                                        || host.native.last_played != last_played
                                        || host.native.collections != collections;
                                    host.native.library = Some(library);
                                    host.native.last_played = last_played;
                                    host.native.collections = collections;
                                    host.native.library_live = true;
                                }
                                match read.session {
                                    // A session read before an action landed is older than its answer.
                                    Ok(_) if !newest => {}
                                    Ok(session) => {
                                        host.native.session = Some(session);
                                        host.session_seq = seq;
                                    }
                                    Err(e) => host.native.error = Some(trf("The session could not be read. {}", &[&e])),
                                }
                            }
                            Err(e) => {
                                host.native.pairing_requests.clear();
                                host.native.error = Some(e);
                            }
                        }
                    }
                    if save && !self.library_saves.contains(&id) {
                        self.library_saves.push(id);
                    }
                    if reload_apps {
                        self.load_apps(id);
                    }
                }
                Event::Session { id, action, result } => {
                    if let Some(host) = self.get_mut(id) {
                        host.native.acting = false;
                        if let Ok(session) = &result {
                            host.native.session = Some((**session).clone());
                            host.native_seq += 1;
                            host.action_seq = host.native_seq;
                        }
                    }
                    self.refresh_after_exit(id);
                    out.push(Event::Session { id, action, result });
                }
                Event::Quit { id, result } => {
                    self.refresh_after_exit(id);
                    out.push(Event::Quit { id, result });
                }
                Event::Wake { id, run, result } => {
                    self.poll_one(id);
                    out.push(Event::Wake { id, run, result });
                }
                event @ Event::Sleep { .. } => out.push(event),
                event @ Event::PcPower { .. } => out.push(event),
                Event::PairingAnswer { id, request, name, denied, result } => {
                    // Whatever the answer, that request is over; the device asks again with a new PIN.
                    if let Some(host) = self.get_mut(id) {
                        host.native.pairing_requests.retain(|r| r.id != request);
                    }
                    self.refresh_native(id, false);
                    out.push(Event::PairingAnswer { id, request, name, denied, result });
                }
                event @ Event::SelectRead { .. } => out.push(event),
                event @ Event::StreamMoved { .. } => out.push(event),
                event @ Event::MoveWatched { .. } => out.push(event),
                event @ Event::ContinueHere { .. } => out.push(event),
                Event::HostUpdate { id, name, result } => {
                    if matches!(result, Ok(UpdateStep::Back(_))) {
                        self.poll_one(id);
                    }
                    // Under way: the menu offers no second install until the next read says.
                    if let (Ok(UpdateStep::Started(_)), Some(update)) =
                        (&result, self.get_mut(id).and_then(|host| host.native.update.as_mut()))
                    {
                        update.phase = "installing".into();
                    }
                    out.push(Event::HostUpdate { id, name, result });
                }
                Event::Action { id, result } => {
                    self.refresh_after_exit(id);
                    self.load_apps(id);
                    out.push(Event::Action { id, result });
                }
                Event::Unpaired { id, result } => {
                    // The PC may be gone by now (removed right after): nothing to tell then.
                    if self.get(id).is_some() {
                        // Unpaired, or the PC never knew this device: either way it is unpaired.
                        if now_unpaired(&result) {
                            self.forget_pairing(id);
                            changed = true;
                        }
                        out.push(Event::Unpaired { id, result });
                    }
                }
            }
        }
        (out, changed)
    }

    /// A PC that was just checked and has the native API: read it, and have the classic app
    /// list ready, because a launch goes through that list.
    fn follow_native(&mut self, id: u64) {
        let Some(host) = self.get(id) else { return };
        if host.reach != Reach::Online || !host.paired() || !host.native_api() {
            return;
        }
        let apps = matches!(host.apps, Apps::NotLoaded);
        // Just after an exit the library's running flags change too: read it again every few seconds.
        let recheck = host.library_recheck_due(Instant::now());
        self.refresh_native(id, recheck);
        if apps {
            self.load_apps(id);
        }
    }

    /// A paired PC that was just checked and has no native API: read its app list once per run,
    /// so Home shows its games and a start from Home has the list it goes through.
    fn follow_classic(&mut self, id: u64) {
        let Some(host) = self.get(id) else { return };
        if host.reach == Reach::Online && host.paired() && host.info.is_some() && !host.native_api() && matches!(host.apps, Apps::NotLoaded) {
            self.load_apps(id);
        }
    }

    /// Asks a paired Sunshine-family PC once whether it has Foundation Sunshine's bandwidth test,
    /// so Test speed is offered only where it runs. A request that did not get through is asked
    /// again after the next poll.
    /// Reads the PC's internet play reply while it is reached on its own network and offers
    /// it, at most every `INTERNET_READ_INTERVAL`: the device keeps it for when it is away.
    fn follow_internet_play(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else { return };
        let Some(host) = self.get_mut(id) else { return };
        let Some(info) = host.info.as_ref() else { return };
        let offered = info.paired && info.capabilities.has(Feature::InternetPlay);
        let due = host.internet_read.is_none_or(|at| at.elapsed() >= INTERNET_READ_INTERVAL);
        if host.reach != Reach::Online || !offered || !due || host.via_remote.is_some() {
            return;
        }
        host.internet_read = Some(Instant::now());
        let capabilities = info.capabilities.clone();
        let record = host.connect_record();
        self.spawn("internet-play", move || {
            let result = client_for(&identity, &record, &record.address).and_then(|c| {
                c.internet_play(&capabilities).map(|(text, _)| text).map_err(|e| e.to_string())
            });
            Some(Event::InternetPlay { id, unique_id: record.unique_id.clone(), result })
        });
    }

    fn check_foundation_probe(&mut self, id: u64) {
        let Some(identity) = self.identity.clone() else { return };
        let Some(host) = self.get_mut(id) else { return };
        let candidate = host.info.as_ref().is_some_and(|i| {
            !i.capabilities.has(Feature::PyroWaveBandwidthProbe) && crate::network_test::has_foundation_probe(i)
        });
        if host.reach != Reach::Online || !candidate || host.foundation_probe.is_some() || host.probing_foundation {
            return;
        }
        host.probing_foundation = true;
        let record = host.connect_record();
        self.spawn("network-capabilities", move || {
            let found = match client_for(&identity, &record, &record.address).map(|c| c.network_capabilities()) {
                Ok(Ok(capabilities)) => match capabilities.probe_bytes() {
                    Ok(_) => Some(true),
                    Err(e) => {
                        eprintln!("nightjar: the PC's bandwidth test is not offered: {e}");
                        Some(false)
                    }
                },
                Ok(Err(nightjar_core::http::Error::Io(e))) => {
                    eprintln!("nightjar: could not ask the PC for its bandwidth test: {e}");
                    None
                }
                Ok(Err(nightjar_core::http::Error::Cancelled)) => None,
                // 404 and the like: stock Sunshine, Apollo or Vibepollo.
                Ok(Err(e)) => {
                    eprintln!("nightjar: the PC has no bandwidth test of its own: {e}");
                    Some(false)
                }
                Err(e) => {
                    eprintln!("nightjar: could not ask the PC for its bandwidth test: {e}");
                    None
                }
            };
            Some(Event::FoundationProbe { id, unique_id: record.unique_id, found })
        });
    }

    pub(super) fn apply_info(&mut self, id: u64, address: String, result: Result<Box<ServerInfo>, String>) -> bool {
        let mut changed = false;
        let Some(index) = self.list.iter().position(|h| h.id == id) else {
            return false;
        };
        // A newly found address may belong to a host already listed under another one.
        if let Ok(info) = &result {
            if let Some(certificate) = self.link_claims.get(&id) {
                if !info.unique_id.is_empty()
                    && self.list.iter().any(|h| h.id != id && h.record.unique_id == info.unique_id && pins_another(&h.record, certificate))
                {
                    self.drop_link_claiming_known_pc(id);
                    return true;
                }
            }
            if let Some(other) = self
                .list
                .iter()
                .position(|h| h.id != id && !h.record.unique_id.is_empty() && h.record.unique_id == info.unique_id)
            {
                let (keep, drop) = if self.list[other].record.server_certificate.is_some()
                    || !self.list[index].record.server_certificate.is_some()
                {
                    (other, index)
                } else {
                    (index, other)
                };
                let dropped = self.list[drop].record.clone();
                let keep_id = self.list[keep].id;
                let kept = &mut self.list[keep].record;
                // The PC keeps the port it is listed under. It answered on the dropped one's too,
                // so a sighting there is that PC from now on, also after Nightjar restarts.
                let own = http_port(kept.http_port);
                for port in std::iter::once(dropped.http_port).chain(dropped.other_http_ports.iter().copied()).map(http_port) {
                    if port != own && !kept.other_http_ports.contains(&port) {
                        kept.other_http_ports.push(port);
                    }
                }
                kept.manual |= dropped.manual;
                if kept.alias.is_none() {
                    kept.alias = dropped.alias;
                }
                if kept.pace.is_none() { kept.pace = dropped.pace; }
                if kept.wake_macs.is_empty() && kept.wake_broadcasts.is_empty() {
                    kept.wake_macs = dropped.wake_macs;
                    kept.wake_broadcasts = dropped.wake_broadcasts;
                }
                for mac in dropped.own_wake_macs {
                    if !kept.own_wake_macs.contains(&mac) {
                        kept.own_wake_macs.push(mac);
                    }
                }
                if kept.settings.is_none() {
                    kept.settings = dropped.settings;
                }
                for (key, settings) in dropped.app_settings {
                    kept.app_settings.entry(key).or_insert(settings);
                }
                kept.quit_on_leave.extend(dropped.quit_on_leave);
                for a in std::iter::once(dropped.address).chain(dropped.other_addresses) {
                    if a != kept.address && !kept.other_addresses.contains(&a) {
                        kept.other_addresses.push(a);
                    }
                }
                if kept.remote_address.is_none() {
                    kept.remote_address = dropped.remote_address;
                }
                self.list.remove(drop);
                changed = true;
                let index = self.list.iter().position(|h| h.id == keep_id).unwrap_or(0);
                return self.store_info(index, address, result) || changed;
            }
        }
        changed |= self.store_info(index, address, result);
        changed
    }

    fn store_info(&mut self, index: usize, address: String, result: Result<Box<ServerInfo>, String>) -> bool {
        let host = &mut self.list[index];
        host.polling = false;
        let mut changed = false;
        match result {
            Ok(info) => {
                host.reach = Reach::Online;
                host.error = None;
                // The remote address answers only when none of the PC's own did: requests go
                // there for now, but the PC stays under its own, so the next poll at home finds
                // it on the LAN again rather than through the router.
                let remote = away_address(&host.record, &address);
                host.via_remote = (remote && host.record.address != address && host.record.pinned_address.is_none())
                    .then(|| address.clone());
                // A pinned PC keeps its address: an answer from another one was still in flight
                // when the user pinned it.
                if !remote && host.record.address != address && host.record.pinned_address.is_none() {
                    let old = std::mem::replace(&mut host.record.address, address.clone());
                    host.record.other_addresses.retain(|a| *a != address);
                    // An older build kept the remote address as the PC's own: it stays the remote one.
                    if !host.record.other_addresses.contains(&old) && host.record.remote_address.as_ref() != Some(&old) {
                        host.record.other_addresses.push(old);
                    }
                    changed = true;
                }
                if host.record.unique_id != info.unique_id {
                    // Another PC answers at this address: what the old one said and allowed is not its.
                    if !host.record.unique_id.is_empty() {
                        host.record.notices.clear();
                        host.record.other_http_ports.clear();
                        host.record.full_chroma = Default::default();
                    }
                    host.native = Native::default();
                    host.set_classic_apps(None);
                    host.native_epoch += 1;
                    host.foundation_probe = None;
                }
                if host.record.unique_id != info.unique_id || host.record.name != info.hostname {
                    host.record.unique_id = info.unique_id.clone();
                    host.record.name = info.hostname.clone();
                    changed = true;
                }
                if host.record.https_port != info.https_port {
                    host.record.https_port = info.https_port;
                    changed = true;
                }
                if host.record.mac.is_none() && info.mac_address.is_some() {
                    host.record.mac = info.mac_address.clone();
                    changed = true;
                }
                // The PC's own ExternalIP replaces a STUN answer (Moonlight V+ PC's NvComputer).
                if let Some(external) = info.external_ip.as_deref().and_then(public_ipv4) {
                    if host.record.remote_address.as_deref() != Some(external.as_str()) {
                        host.record.remote_address = Some(external);
                        changed = true;
                    }
                }
                // The host forgot this client: drop the pin so the user can pair again.
                if !info.paired && host.record.server_certificate.is_some() {
                    host.record.server_certificate = None;
                    host.apps = Apps::NotLoaded;
                    // Its grants (folders, files, USB, co-op) went with the pairing; a read in flight is dropped.
                    host.native = Native::default();
                    host.set_classic_apps(None);
                    host.native_epoch += 1;
                    changed = true;
                }
                crate::logfile::add_secrets(&host.record);
                // Serverinfo alone says nothing of the native features: keep the last native read's.
                let mut info = *info;
                merge_native(&mut info, host.native.capabilities.as_ref());
                host.info = Some(info);
            }
            Err(e) => {
                host.reach = Reach::Offline;
                host.error = Some(e);
            }
        }
        host.sync_classic();
        changed
    }

    /// Takes in an mDNS sighting. True when a listed PC's stored address changed.
    pub(super) fn apply_discovery(&mut self, name: String, addresses: Vec<IpAddr>, port: u16) -> bool {
        let Some(first) = addresses.first() else {
            return false;
        };
        let texts: Vec<String> = addresses.iter().map(IpAddr::to_string).collect();
        // A PC is an address and a port: another port on the same address is another PC (a
        // second host on that machine, or one added by hand with its own port), until it
        // answers with a listed PC's unique ID.
        let port = http_port(port);
        let at_address = |h: &Host| texts.contains(&h.record.address) || h.record.other_addresses.iter().any(|a| texts.contains(a));
        let listed = self
            .list
            .iter()
            .position(|h| http_port(h.record.http_port) == port && at_address(h))
            .or_else(|| self.list.iter().position(|h| h.record.other_http_ports.contains(&port) && at_address(h)));
        if let Some(host) = listed.map(|at| &mut self.list[at]) {
            if host.record.name.is_empty() {
                host.record.name = name;
                crate::logfile::add_secrets(&host.record);
            }
            // Seen on this network: its own address reaches it again, not the router's.
            host.via_remote = None;
            let record = &mut host.record;
            if record.pinned_address.is_none() && record.remote_address.as_ref() == Some(&record.address) {
                if let Some(at) = record.other_addresses.iter().position(|a| texts.contains(a)) {
                    record.address = record.other_addresses.remove(at);
                    return true;
                }
            }
            return false;
        }
        let id = self.insert(HostRecord {
            name,
            address: first.to_string(),
            other_addresses: texts[1..].to_vec(),
            http_port: port,
            ..Default::default()
        });
        if first.is_ipv4() {
            self.find_remote_address(id);
        }
        self.poll_one(id);
        false
    }

    /// Takes a STUN answer as the PC's remote address. The PC's own ExternalIP, once it has
    /// answered with one, stays.
    fn apply_remote_address(&mut self, id: u64, address: Ipv4Addr) -> bool {
        let Some(host) = self.get_mut(id) else { return false };
        if host.info.as_ref().and_then(|i| i.external_ip.as_deref()).and_then(public_ipv4).is_some() {
            return false;
        }
        let address = address.to_string();
        if host.record.remote_address.as_deref() == Some(address.as_str()) {
            return false;
        }
        host.record.remote_address = Some(address);
        crate::logfile::add_secrets(&host.record);
        true
    }
}
