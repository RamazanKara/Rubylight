//! The FFI side of the probe. It walks `IADLX3DSettingsServices` (and its 1, 2 and 3 versions) and
//! copies what each getter says into `model::GroupRaw`; `model` decides what it means.
//!
//! Setter slots are bound as callable functions only for the settings `control` may write; every
//! other slot is a plain `usize`, so it cannot be called from here. Nothing in `probe` writes:
//! only `AdlxBackend::write` reaches a setter.
//!
//! The vtables are bound by hand from AMD's public C ABI, SDK 2.0.0.125:
//! https://github.com/GPUOpen-LibrariesAndSDKs/ADLX/tree/32b5a740d42295c5dfe9026b9f52683da0f3af91/SDK/Include
//! (`I3DSettings.h`, `I3DSettings1.h`, `I3DSettings2.h`, `I3DSettings3.h`, `ISystem.h`,
//! `ISystem2.h`). Loading, initialisation and reference counting reuse `telemetry`.
use super::{
    control::{BackendError, RadeonBackend},
    model::*,
};
use crate::telemetry::{
    ADLX_VERSION, BaseVtable, GetBool, GetGpuInterface, GetInt, GetInterface, Handle, Interface,
    ListVtable, system_library,
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    ffi::{CStr, c_char},
    ptr,
};

/// A setter slot that is not bound. It is an integer so it cannot be called.
type Unbound = usize;
type SetBool = unsafe extern "system" fn(Handle, u8) -> i32;
type SetInt = unsafe extern "system" fn(Handle, i32) -> i32;
type GetRange = unsafe extern "system" fn(Handle, *mut RawRange) -> i32;
type GetText = unsafe extern "system" fn(Handle, *mut *const c_char) -> i32;
#[repr(C)]
#[derive(Default)]
struct RawRange {
    min: i32,
    max: i32,
    step: i32,
}

#[repr(C)]
struct SystemVt {
    hybrid: Unbound,
    gpus: GetInterface,
    /// GetDisplaysServices, GetDesktopsServices, GetGPUsChangedHandling, EnableLog.
    unused: [usize; 5],
    services_3d: GetInterface,
}
#[repr(C)]
struct ServicesVt {
    base: BaseVtable,
    anti_lag: GetGpuInterface,
    chill: GetGpuInterface,
    boost: GetGpuInterface,
    image_sharpening: GetGpuInterface,
    enhanced_sync: GetGpuInterface,
    vsync: GetGpuInterface,
    frame_rate_target: GetGpuInterface,
    anti_aliasing: GetGpuInterface,
    morphological_aa: GetGpuInterface,
    anisotropic_filtering: GetGpuInterface,
    tessellation: GetGpuInterface,
    /// No GPU argument.
    rsr: GetInterface,
    reset_shader_cache: GetGpuInterface,
    changed_handling: Unbound,
}
#[repr(C)]
struct Services1Vt {
    base: ServicesVt,
    /// No GPU argument.
    afmf: GetInterface,
}
#[repr(C)]
struct Services2Vt {
    base: Services1Vt,
    image_sharpen_desktop: GetGpuInterface,
}
#[repr(C)]
struct Services3Vt {
    base: Services2Vt,
    fidelityfx_super_resolution: GetGpuInterface,
    fidelityfx_frame_gen: GetGpuInterface,
}

#[repr(C)]
struct GpuVt {
    base: BaseVtable,
    /// VendorId, ASICFamilyType, Type, IsExternal.
    unused1: [usize; 4],
    name: GetText,
    driver_path: GetText,
    /// PNPString .. SubSystemVendorId.
    unused2: [usize; 9],
    unique_id: GetInt,
}
#[repr(C)]
struct Gpu2Vt {
    base: GpuVt,
    /// PCIBusType .. AbortPowerOff, IsSupportedApplicationList, GetApplications.
    unused: [usize; 10],
    release_date: Unbound,
    edition: GetText,
    software_version: GetText,
    driver_version: GetText,
    windows_driver_version: GetText,
}

#[repr(C)]
struct Head {
    base: BaseVtable,
    supported: GetBool,
}
#[repr(C)]
struct ToggleVt {
    head: Head,
    enabled: GetBool,
    set_enabled: SetBool,
}
#[repr(C)]
struct AntiLag1Vt {
    base: ToggleVt,
    level: GetInt,
    set_level: Unbound,
}
#[repr(C)]
struct ChillVt {
    head: Head,
    enabled: GetBool,
    range: GetRange,
    min_fps: GetInt,
    max_fps: GetInt,
    set_enabled: SetBool,
    set_min_fps: SetInt,
    set_max_fps: SetInt,
}
/// Boost, image sharpening and frame rate target control share this layout.
#[repr(C)]
struct RangedVt {
    head: Head,
    enabled: GetBool,
    range: GetRange,
    value: GetInt,
    set_enabled: SetBool,
    set_value: SetInt,
}
#[repr(C)]
struct VsyncVt {
    head: Head,
    enabled: GetBool,
    mode: GetInt,
    set_mode: SetInt,
}
#[repr(C)]
struct AntiAliasingVt {
    head: Head,
    mode: GetInt,
    level: GetInt,
    method: GetInt,
    set_mode: SetInt,
    set_level: SetInt,
    set_method: SetInt,
}
#[repr(C)]
struct AnisotropicVt {
    head: Head,
    enabled: GetBool,
    level: GetInt,
    set_enabled: SetBool,
    set_level: SetInt,
}
#[repr(C)]
struct TessellationVt {
    head: Head,
    mode: GetInt,
    level: GetInt,
    set_mode: SetInt,
    set_level: SetInt,
}
#[repr(C)]
struct RsrVt {
    head: Head,
    enabled: GetBool,
    set_enabled: Unbound,
    range: GetRange,
    sharpness: GetInt,
    set_sharpness: SetInt,
}
#[repr(C)]
struct Afmf1Vt {
    base: ToggleVt,
    algorithm_supported: GetBool,
    algorithm: GetInt,
    set_algorithm: Unbound,
    search_mode: GetInt,
    set_search_mode: SetInt,
    performance_mode: GetInt,
    set_performance_mode: SetInt,
    fast_motion: GetInt,
    set_fast_motion: SetInt,
}
#[repr(C)]
struct FrameGenVt {
    base: BaseVtable,
    ratios: GetInterface,
    ratio: GetInt,
    supported: GetBool,
    enabled: GetBool,
    set_enabled: Unbound,
    set_ratio: Unbound,
}
#[repr(C)]
struct RatioOptionVt {
    base: BaseVtable,
    ratio: GetInt,
}

fn code(result: i32) -> Query<()> {
    if result == 0 {
        Ok(())
    } else {
        Err(Fault::Code(result))
    }
}
fn get_bool(i: &Interface, f: GetBool) -> Query<bool> {
    let mut value = 0u8;
    // SAFETY: `f` is a getter of the live interface `i`; the output is a live scalar.
    code(unsafe { f(i.0, &mut value) }).map(|()| value != 0)
}
fn get_int(i: &Interface, f: GetInt) -> Query<i32> {
    let mut value = 0;
    // SAFETY: `f` is a getter of the live interface `i`; the output is a live scalar.
    code(unsafe { f(i.0, &mut value) }).map(|()| value)
}
fn get_range(i: &Interface, f: GetRange) -> Query<Range> {
    let mut raw = RawRange::default();
    // SAFETY: `f` is a range getter of the live interface `i`; the output is a live ADLX_IntRange.
    code(unsafe { f(i.0, &mut raw) }).map(|()| Range {
        min: raw.min.into(),
        max: raw.max.into(),
        step: raw.step.into(),
    })
}
fn get_text(i: &Interface, f: GetText) -> Option<String> {
    let mut text = ptr::null();
    // SAFETY: `f` is a string getter of the live interface `i`; the string it returns is owned by
    // ADLX and stays valid while `i` lives, and is copied here.
    unsafe {
        (f(i.0, &mut text) == 0 && !text.is_null())
            .then(|| CStr::from_ptr(text).to_string_lossy().into_owned())
    }
}
fn obtain(call: impl FnOnce(&mut Handle) -> i32) -> Query<Interface> {
    let mut out = ptr::null_mut();
    let result = call(&mut out);
    Interface::returned(result, out).ok_or(Fault::Code(if result == 0 { 3 } else { result }))
}
fn query(i: &Interface, name: &str) -> Query<Interface> {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    obtain(|out| {
        // SAFETY: Every ADLX interface starts with BaseVtable; the name is terminated and the
        // output is live.
        unsafe { (i.vtable::<BaseVtable>().query)(i.0, name.as_ptr(), out) }
    })
}

/// An interface that was asked for, and what IsSupported said.
struct Open {
    obtained: Query<()>,
    supported: Option<Query<bool>>,
    interface: Option<Interface>,
}
impl Open {
    fn new(obtained: Query<Interface>, supported: impl FnOnce(&Interface) -> Query<bool>) -> Self {
        match obtained {
            Ok(interface) => Self {
                obtained: Ok(()),
                supported: Some(supported(&interface)),
                interface: Some(interface),
            },
            Err(fault) => Self {
                obtained: Err(fault),
                supported: None,
                interface: None,
            },
        }
    }
    /// Every interface but FrameGen starts with `Head`.
    fn standard(obtained: Query<Interface>) -> Self {
        Self::new(obtained, |i| {
            // SAFETY: `i` is an IADLX3D* interface whose first method after IADLXInterface is IsSupported.
            get_bool(i, unsafe { i.vtable::<Head>() }.supported)
        })
    }
    /// An interface reached through QueryInterface on another one.
    fn extension(parent: &Open, name: &str) -> Self {
        match (&parent.interface, &parent.obtained) {
            (Some(parent), _) => Self::standard(query(parent, name)),
            (None, Err(fault)) => Self::standard(Err(*fault)),
            (None, Ok(())) => Self::standard(Err(Fault::Missing)),
        }
    }
    /// The interface, only when ADLX said the feature is supported. Reading an unsupported
    /// feature proves nothing, so the getters are not called otherwise.
    fn ready(&self) -> Option<&Interface> {
        match self.supported {
            Some(Ok(true)) => self.interface.as_ref(),
            _ => None,
        }
    }
    fn group(
        self,
        group: &'static str,
        interface: &'static str,
        fields: Vec<FieldRaw>,
    ) -> GroupRaw {
        GroupRaw {
            group,
            interface,
            obtained: self.obtained,
            supported: self.supported,
            fields,
        }
    }
}

/// Stands in for a value that was not read because the feature is unavailable. `classify` stops
/// before it looks at fields of an unavailable interface.
const UNREAD: Fault = Fault::Missing;
fn field(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    value: Option<Query<Value>>,
) -> FieldRaw {
    FieldRaw {
        id,
        label,
        kind,
        unit: None,
        value,
        range: None,
        options: vec![],
        options_queried: false,
        note: None,
    }
}
fn toggle(
    id: &'static str,
    label: &'static str,
    i: Option<&Interface>,
    f: impl FnOnce() -> GetBool,
) -> FieldRaw {
    field(
        id,
        label,
        Kind::Toggle,
        Some(i.map_or(Err(UNREAD), |i| get_bool(i, f()).map(Value::Bool))),
    )
}
fn integer(
    id: &'static str,
    label: &'static str,
    unit: &'static str,
    i: Option<&Interface>,
    value: impl FnOnce() -> GetInt,
    range: impl FnOnce() -> GetRange,
) -> FieldRaw {
    let mut f = field(
        id,
        label,
        Kind::Integer,
        Some(i.map_or(Err(UNREAD), |i| {
            get_int(i, value()).map(|v| Value::Int(v.into()))
        })),
    );
    f.unit = Some(unit);
    f.range = Some(i.map_or(Err(UNREAD), |i| get_range(i, range())));
    f
}
fn choice(
    id: &'static str,
    label: &'static str,
    items: EnumItems,
    i: Option<&Interface>,
    f: impl FnOnce() -> GetInt,
) -> FieldRaw {
    let mut out = field(
        id,
        label,
        Kind::Choice,
        Some(i.map_or(Err(UNREAD), |i| {
            get_int(i, f()).map(|raw| Value::Choice(enum_name(items, raw)))
        })),
    );
    out.options = enum_options(items);
    out
}
fn action(id: &'static str, label: &'static str) -> FieldRaw {
    field(id, label, Kind::Action, None)
}

/// Everything the probe needs to reach the 3D settings of one GPU.
struct Ctx<'a> {
    gpu: &'a Interface,
    v0: &'a Interface,
    v1: Option<&'a Interface>,
    v2: Option<&'a Interface>,
    v3: Option<&'a Interface>,
}
impl Ctx<'_> {
    fn with_gpu(&self, pick: impl FnOnce(&ServicesVt) -> GetGpuInterface) -> Query<Interface> {
        // SAFETY: `v0` is IADLX3DSettingsServices (or a later version, which shares its prefix).
        let call = pick(unsafe { self.v0.vtable::<ServicesVt>() });
        // SAFETY: `call` belongs to the live services object; the GPU and output are live.
        obtain(|out| unsafe { call(self.v0.0, self.gpu.0, out) })
    }
    fn afmf(&self) -> Query<Interface> {
        let Some(v1) = self.v1 else {
            return Err(Fault::Missing);
        };
        // SAFETY: `v1` was returned by QueryInterface for IADLX3DSettingsServices1.
        let call = unsafe { v1.vtable::<Services1Vt>() }.afmf;
        // SAFETY: `call` belongs to the live services object; the output is live.
        obtain(|out| unsafe { call(v1.0, out) })
    }
    fn sharpen_desktop(&self) -> Query<Interface> {
        let Some(v2) = self.v2 else {
            return Err(Fault::Missing);
        };
        // SAFETY: `v2` was returned by QueryInterface for IADLX3DSettingsServices2.
        let call = unsafe { v2.vtable::<Services2Vt>() }.image_sharpen_desktop;
        // SAFETY: `call` belongs to the live services object; the GPU and output are live.
        obtain(|out| unsafe { call(v2.0, self.gpu.0, out) })
    }
    fn fidelityfx(&self, frame_gen: bool) -> Query<Interface> {
        let Some(v3) = self.v3 else {
            return Err(Fault::Missing);
        };
        // SAFETY: `v3` was returned by QueryInterface for IADLX3DSettingsServices3.
        let table = unsafe { v3.vtable::<Services3Vt>() };
        let call = if frame_gen {
            table.fidelityfx_frame_gen
        } else {
            table.fidelityfx_super_resolution
        };
        // SAFETY: `call` belongs to the live services object; the GPU and output are live.
        obtain(|out| unsafe { call(v3.0, self.gpu.0, out) })
    }
    fn rsr(&self) -> Query<Interface> {
        // SAFETY: `v0` is IADLX3DSettingsServices or later, whose RSR getter takes no GPU.
        let call = unsafe { self.v0.vtable::<ServicesVt>() }.rsr;
        // SAFETY: `call` belongs to the live services object; the output is live.
        obtain(|out| unsafe { call(self.v0.0, out) })
    }
}

macro_rules! table {
    ($open:expr, $ty:ty) => {{
        let ready = $open.ready();
        // SAFETY: the interface was returned by the Get call whose ADLX type has this layout.
        let table: Option<&$ty> = ready.map(|i| unsafe { i.vtable::<$ty>() });
        (ready, table)
    }};
}

fn groups(c: &Ctx) -> Vec<GroupRaw> {
    let mut out = vec![];

    let anti_lag = Open::standard(c.with_gpu(|s| s.anti_lag));
    let (i, t) = table!(anti_lag, ToggleVt);
    let fields = vec![toggle("anti_lag.enabled", "Anti-Lag", i, || {
        t.unwrap().enabled
    })];
    let level = Open::extension(&anti_lag, "IADLX3DAntiLag1");
    let (li, lt) = table!(level, AntiLag1Vt);
    let mut level_field = choice(
        "anti_lag.level",
        "Anti-Lag: driver Anti-Lag or Anti-Lag Next",
        ANTILAG_STATE,
        li,
        || lt.unwrap().level,
    );
    level_field.note = Some(
        "Anti-Lag 2 is an in-game SDK integration and is not exposed by ADLX; this selects the driver's Anti-Lag or Anti-Lag Next".into(),
    );
    out.push(anti_lag.group("anti_lag", "IADLX3DAntiLag", fields));
    out.push(level.group("anti_lag", "IADLX3DAntiLag1", vec![level_field]));

    let chill = Open::standard(c.with_gpu(|s| s.chill));
    let (i, t) = table!(chill, ChillVt);
    let fields = vec![
        toggle("chill.enabled", "Radeon Chill", i, || t.unwrap().enabled),
        integer(
            "chill.min_fps",
            "Radeon Chill: minimum FPS",
            "fps",
            i,
            || t.unwrap().min_fps,
            || t.unwrap().range,
        ),
        integer(
            "chill.max_fps",
            "Radeon Chill: maximum FPS",
            "fps",
            i,
            || t.unwrap().max_fps,
            || t.unwrap().range,
        ),
    ];
    out.push(chill.group("chill", "IADLX3DChill", fields));

    let boost = Open::standard(c.with_gpu(|s| s.boost));
    let (i, t) = table!(boost, RangedVt);
    let fields = vec![
        toggle("boost.enabled", "Radeon Boost", i, || t.unwrap().enabled),
        integer(
            "boost.min_resolution",
            "Radeon Boost: minimum resolution",
            "%",
            i,
            || t.unwrap().value,
            || t.unwrap().range,
        ),
    ];
    out.push(boost.group("boost", "IADLX3DBoost", fields));

    let sharpening = Open::standard(c.with_gpu(|s| s.image_sharpening));
    let (i, t) = table!(sharpening, RangedVt);
    let fields = vec![
        toggle(
            "image_sharpening.enabled",
            "Radeon Image Sharpening",
            i,
            || t.unwrap().enabled,
        ),
        integer(
            "image_sharpening.sharpness",
            "Radeon Image Sharpening: sharpness",
            "%",
            i,
            || t.unwrap().value,
            || t.unwrap().range,
        ),
    ];
    out.push(sharpening.group("image_sharpening", "IADLX3DImageSharpening", fields));

    let sync = Open::standard(c.with_gpu(|s| s.enhanced_sync));
    let (i, t) = table!(sync, ToggleVt);
    let fields = vec![toggle("enhanced_sync.enabled", "Enhanced Sync", i, || {
        t.unwrap().enabled
    })];
    out.push(sync.group("enhanced_sync", "IADLX3DEnhancedSync", fields));

    let vsync = Open::standard(c.with_gpu(|s| s.vsync));
    let (i, t) = table!(vsync, VsyncVt);
    let fields = vec![
        toggle("vsync.enabled", "Wait for Vertical Refresh", i, || {
            t.unwrap().enabled
        }),
        choice(
            "vsync.mode",
            "Wait for Vertical Refresh: mode",
            WAIT_FOR_VERTICAL_REFRESH,
            i,
            || t.unwrap().mode,
        ),
    ];
    out.push(vsync.group("vsync", "IADLX3DWaitForVerticalRefresh", fields));

    let target = Open::standard(c.with_gpu(|s| s.frame_rate_target));
    let (i, t) = table!(target, RangedVt);
    let fields = vec![
        toggle(
            "frame_rate_target.enabled",
            "Frame Rate Target Control",
            i,
            || t.unwrap().enabled,
        ),
        integer(
            "frame_rate_target.fps",
            "Frame Rate Target Control: target",
            "fps",
            i,
            || t.unwrap().value,
            || t.unwrap().range,
        ),
    ];
    out.push(target.group("frame_rate_target", "IADLX3DFrameRateTargetControl", fields));

    let afmf = Open::standard(c.afmf());
    let (i, t) = table!(afmf, ToggleVt);
    let fields = vec![toggle("afmf.enabled", "AMD Fluid Motion Frames", i, || {
        t.unwrap().enabled
    })];
    let detail = Open::extension(&afmf, "IADLX3DAMDFluidMotionFrames1");
    let (di, dt) = table!(detail, Afmf1Vt);
    let mut algorithm = choice(
        "afmf.algorithm",
        "AFMF: algorithm",
        AFMF_ALGORITHM,
        di,
        || dt.unwrap().algorithm,
    );
    algorithm.note = Some(
        match di.zip(dt).map(|(i, t)| get_bool(i, t.algorithm_supported)) {
            Some(Ok(true)) => "IsSupportedAlgorithm is true".into(),
            Some(Ok(false)) => {
                "IsSupportedAlgorithm is false: the algorithm choice is not offered on this GPU"
                    .into()
            }
            Some(Err(fault)) => format!("IsSupportedAlgorithm failed: {}", fault.describe()),
            None => "IsSupportedAlgorithm not read".into(),
        },
    );
    let fields_detail = vec![
        algorithm,
        choice(
            "afmf.search_mode",
            "AFMF: search mode",
            AFMF_SEARCH_MODE,
            di,
            || dt.unwrap().search_mode,
        ),
        choice(
            "afmf.performance_mode",
            "AFMF: performance mode",
            AFMF_PERFORMANCE_MODE,
            di,
            || dt.unwrap().performance_mode,
        ),
        choice(
            "afmf.fast_motion_response",
            "AFMF: fast motion response",
            AFMF_FAST_MOTION,
            di,
            || dt.unwrap().fast_motion,
        ),
    ];
    out.push(afmf.group("afmf", "IADLX3DAMDFluidMotionFrames", fields));
    out.push(detail.group("afmf", "IADLX3DAMDFluidMotionFrames1", fields_detail));

    let rsr = Open::standard(c.rsr());
    let (i, t) = table!(rsr, RsrVt);
    let fields = vec![
        toggle("rsr.enabled", "Radeon Super Resolution", i, || {
            t.unwrap().enabled
        }),
        integer(
            "rsr.sharpness",
            "Radeon Super Resolution: sharpness",
            "%",
            i,
            || t.unwrap().sharpness,
            || t.unwrap().range,
        ),
    ];
    out.push(rsr.group("rsr", "IADLX3DRadeonSuperResolution", fields));

    let aa = Open::standard(c.with_gpu(|s| s.anti_aliasing));
    let (i, t) = table!(aa, AntiAliasingVt);
    let fields = vec![
        choice(
            "anti_aliasing.mode",
            "Anti-aliasing: mode",
            AA_MODE,
            i,
            || t.unwrap().mode,
        ),
        choice(
            "anti_aliasing.level",
            "Anti-aliasing: level",
            AA_LEVEL,
            i,
            || t.unwrap().level,
        ),
        choice(
            "anti_aliasing.method",
            "Anti-aliasing: method",
            AA_METHOD,
            i,
            || t.unwrap().method,
        ),
    ];
    out.push(aa.group("anti_aliasing", "IADLX3DAntiAliasing", fields));

    let mlaa = Open::standard(c.with_gpu(|s| s.morphological_aa));
    let (i, t) = table!(mlaa, ToggleVt);
    let fields = vec![toggle(
        "morphological_aa.enabled",
        "Morphological anti-aliasing",
        i,
        || t.unwrap().enabled,
    )];
    out.push(mlaa.group(
        "morphological_aa",
        "IADLX3DMorphologicalAntiAliasing",
        fields,
    ));

    let af = Open::standard(c.with_gpu(|s| s.anisotropic_filtering));
    let (i, t) = table!(af, AnisotropicVt);
    let fields = vec![
        toggle(
            "anisotropic_filtering.enabled",
            "Anisotropic filtering",
            i,
            || t.unwrap().enabled,
        ),
        choice(
            "anisotropic_filtering.level",
            "Anisotropic filtering: level",
            AF_LEVEL,
            i,
            || t.unwrap().level,
        ),
    ];
    out.push(af.group(
        "anisotropic_filtering",
        "IADLX3DAnisotropicFiltering",
        fields,
    ));

    let tess = Open::standard(c.with_gpu(|s| s.tessellation));
    let (i, t) = table!(tess, TessellationVt);
    let fields = vec![
        choice(
            "tessellation.mode",
            "Tessellation: mode",
            TESSELLATION_MODE,
            i,
            || t.unwrap().mode,
        ),
        choice(
            "tessellation.level",
            "Tessellation: level",
            TESSELLATION_LEVEL,
            i,
            || t.unwrap().level,
        ),
    ];
    out.push(tess.group("tessellation", "IADLX3DTessellation", fields));

    let reset = Open::standard(c.with_gpu(|s| s.reset_shader_cache));
    let mut run = action("reset_shader_cache.run", "Reset shader cache");
    run.note = Some("Action: nothing to read; the probe does not invoke it".into());
    out.push(reset.group("reset_shader_cache", "IADLX3DResetShaderCache", vec![run]));

    let desktop = Open::standard(c.sharpen_desktop());
    let (i, t) = table!(desktop, ToggleVt);
    let fields = vec![toggle(
        "image_sharpen_desktop.enabled",
        "Image Sharpening on the desktop",
        i,
        || t.unwrap().enabled,
    )];
    out.push(desktop.group(
        "image_sharpen_desktop",
        "IADLX3DImageSharpenDesktop",
        fields,
    ));

    let fsr = Open::standard(c.fidelityfx(false));
    let (i, t) = table!(fsr, ToggleVt);
    let fields = vec![toggle(
        "fidelityfx_super_resolution.enabled",
        "FidelityFX Super Resolution upgrade",
        i,
        || t.unwrap().enabled,
    )];
    out.push(fsr.group(
        "fidelityfx_super_resolution",
        "IADLX3DFidelityFXSuperResolution",
        fields,
    ));

    let frame_gen = Open::new(c.fidelityfx(true), |i| {
        // SAFETY: `i` is IADLX3DFidelityFXFrameGenUpgrade, whose IsSupported follows GetAvailableRatios and GetRatio.
        get_bool(i, unsafe { i.vtable::<FrameGenVt>() }.supported)
    });
    let (i, t) = table!(frame_gen, FrameGenVt);
    let mut ratio = field(
        "fidelityfx_frame_gen.ratio",
        "FidelityFX frame generation upgrade: ratio",
        Kind::Choice,
        Some(i.zip(t).map_or(Err(UNREAD), |(i, t)| {
            get_int(i, t.ratio).map(|raw| Value::Choice(enum_name(FFX_FRAME_GEN_RATIO, raw)))
        })),
    );
    if let Some((i, t)) = i.zip(t) {
        ratio.options_queried = true;
        ratio.options = frame_gen_ratios(i, t);
    }
    let fields = vec![
        toggle(
            "fidelityfx_frame_gen.enabled",
            "FidelityFX frame generation upgrade",
            i,
            || t.unwrap().enabled,
        ),
        ratio,
    ];
    out.push(frame_gen.group(
        "fidelityfx_frame_gen",
        "IADLX3DFidelityFXFrameGenUpgrade",
        fields,
    ));
    out
}

/// The ratios ADLX lists for this GPU.
fn frame_gen_ratios(i: &Interface, t: &FrameGenVt) -> Vec<ChoiceOption> {
    let Ok(list) = obtain(|out| {
        // SAFETY: `t` is the vtable of the live frame generation interface; the output is live.
        unsafe { (t.ratios)(i.0, out) }
    }) else {
        return vec![];
    };
    // SAFETY: GetAvailableRatios returns an IADLX3DFidelityFXFrameGenUpgradeRatioOptionList, which
    // has the common ADLX list layout.
    let table = unsafe { list.vtable::<ListVtable>() };
    // SAFETY: The list is live.
    let count = unsafe { (table.size)(list.0) };
    (0..count)
        .filter_map(|index| {
            let item = obtain(|out| {
                // SAFETY: index is within the list; At_OptionList acquires a reference into `out`.
                unsafe { (table.at)(list.0, index, out) }
            })
            .ok()?;
            // SAFETY: the item is an IADLX3DFidelityFXFrameGenUpgradeRatioOption.
            let raw = get_int(&item, unsafe { item.vtable::<RatioOptionVt>() }.ratio).ok()?;
            Some(ChoiceOption {
                value: enum_name(FFX_FRAME_GEN_RATIO, raw),
                raw,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize)]
pub struct GpuReport {
    pub id: i32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_path: Option<String>,
    pub driver: DriverInfo,
    pub settings: Vec<Setting>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct DriverInfo {
    /// `IADLXGPU2::AMDSoftwareVersion`, for example the Adrenalin release.
    pub amd_software_version: Option<String>,
    pub amd_software_edition: Option<String>,
    /// `IADLXGPU2::DriverVersion`.
    pub driver_version: Option<String>,
    /// `IADLXGPU2::AMDWindowsDriverVersion`.
    pub windows_driver_version: Option<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct AdlxInfo {
    /// Installed runtime (`amdadlx64.dll`), from `ADLXQueryFullVersion`.
    pub runtime_version: Option<String>,
    /// What the probe asked `ADLXInitialize` for.
    pub requested_version: String,
    /// The headers the vtables were bound from.
    pub header_sdk_version: &'static str,
    pub initialize: String,
    /// The highest `IADLX3DSettingsServices` version the runtime answered for.
    pub services_interface: Option<&'static str>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub adlx: AdlxInfo,
    pub gpus: Vec<GpuReport>,
}

fn version_text(full: u64) -> String {
    format!(
        "{}.{}.{}.{}",
        full >> 48,
        (full >> 32) & 0xffff,
        (full >> 16) & 0xffff,
        full & 0xffff
    )
}

/// Only one session talks to ADLX at a time, so two threads never race a write.
static ADLX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The ADLX runtime stays loaded, and initialized, for the life of the process: calling
/// `ADLXTerminate` after each session faulted (access violation) within a few hundred
/// initialize/terminate cycles on AMD Software 26.9.2. A later `ADLXInitialize` returns
/// ADLX_ALREADY_INITIALIZED, which is fine, and nothing here ends telemetry's ADLX under it.
static LIBRARY: std::sync::OnceLock<libloading::Library> = std::sync::OnceLock::new();
pub(super) fn library() -> Result<&'static libloading::Library> {
    if let Some(library) = LIBRARY.get() {
        return Ok(library);
    }
    let loaded = system_library("amdadlx64.dll")?;
    Ok(LIBRARY.get_or_init(|| loaded))
}

/// An initialised ADLX with the 3D settings services and the GPU list.
struct Session {
    // Fields drop in this order: interfaces, then the lock.
    gpus: Vec<(i32, Interface)>,
    v3: Option<Interface>,
    v2: Option<Interface>,
    v1: Option<Interface>,
    services: Interface,
    info: AdlxInfo,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl Session {
    fn open() -> Result<Self> {
        let lock = ADLX_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let library = library()?;
        // SAFETY: These exports and C signatures are specified by AMD's ADLX.h.
        let (initialize, full_version) = unsafe {
            (
                *library
                    .get::<unsafe extern "C" fn(u64, *mut Handle) -> i32>(b"ADLXInitialize\0")?,
                library
                    .get::<unsafe extern "C" fn(*mut u64) -> i32>(b"ADLXQueryFullVersion\0")
                    .ok()
                    .map(|f| *f),
            )
        };
        let runtime_version = full_version.and_then(|query| {
            let mut full = 0u64;
            // SAFETY: The export writes one u64.
            (unsafe { query(&mut full) } == 0).then(|| version_text(full))
        });
        let mut system = ptr::null_mut();
        // SAFETY: The runtime is loaded, the version is the baseline ABI the host already
        // negotiates and `system` is writable.
        let result = unsafe { initialize(ADLX_VERSION, &mut system) };
        if !matches!(result, 0..=2) || system.is_null() {
            bail!("ADLX initialization failed ({})", result_name(result));
        }
        // SAFETY: ADLXInitialize returned IADLXSystem, which stays valid until ADLXTerminate, and
        // this module never calls that.
        let table = unsafe { &**system.cast::<*const SystemVt>() };
        let list = obtain(|out| {
            // SAFETY: The system and output are live; GetGPUs returns IADLXGPUList.
            unsafe { (table.gpus)(system, out) }
        })
        .map_err(|fault| anyhow::anyhow!("GetGPUs: {}", fault.describe()))?;
        let services = obtain(|out| {
            // SAFETY: The system and output are live; Get3DSettingsServices returns IADLX3DSettingsServices.
            unsafe { (table.services_3d)(system, out) }
        })
        .map_err(|fault| anyhow::anyhow!("Get3DSettingsServices: {}", fault.describe()))
        .context("ADLX 3D settings services unavailable")?;
        let v1 = query(&services, "IADLX3DSettingsServices1").ok();
        let v2 = query(&services, "IADLX3DSettingsServices2").ok();
        let v3 = query(&services, "IADLX3DSettingsServices3").ok();
        let services_interface = [
            (v3.is_some(), "IADLX3DSettingsServices3"),
            (v2.is_some(), "IADLX3DSettingsServices2"),
            (v1.is_some(), "IADLX3DSettingsServices1"),
            (true, "IADLX3DSettingsServices"),
        ]
        .into_iter()
        .find_map(|(present, name)| present.then_some(name));

        // SAFETY: The list was returned by GetGPUs as IADLXGPUList.
        let list_table = unsafe { list.vtable::<ListVtable>() };
        // SAFETY: The list is live.
        let count = unsafe { (list_table.size)(list.0) };
        let mut gpus = vec![];
        for index in 0..count {
            let Ok(gpu) = obtain(|out| {
                // SAFETY: index is within the list; At_GPUList acquires a reference into `out`.
                unsafe { (list_table.at)(list.0, index, out) }
            }) else {
                continue;
            };
            // SAFETY: `gpu` is an IADLXGPU.
            let gpu_table = unsafe { gpu.vtable::<GpuVt>() };
            let mut id = 0;
            // SAFETY: The getter belongs to the live GPU; `id` is a live scalar.
            if unsafe { (gpu_table.unique_id)(gpu.0, &mut id) } != 0 {
                continue;
            }
            gpus.push((id, gpu));
        }
        Ok(Self {
            gpus,
            v3,
            v2,
            v1,
            services,
            info: AdlxInfo {
                runtime_version,
                requested_version: version_text(ADLX_VERSION),
                header_sdk_version: "2.0.0.125",
                initialize: result_name(result),
                services_interface,
            },
            _lock: lock,
        })
    }

    fn ctx<'a>(&'a self, gpu: &'a Interface) -> Ctx<'a> {
        Ctx {
            gpu,
            v0: &self.services,
            v1: self.v1.as_ref(),
            v2: self.v2.as_ref(),
            v3: self.v3.as_ref(),
        }
    }

    fn settings(&self, gpu: &Interface) -> Vec<Setting> {
        let mut settings: Vec<Setting> = groups(&self.ctx(gpu))
            .iter()
            .flat_map(classify_group)
            .collect();
        apply_dependencies(&mut settings);
        settings
    }

    fn gpu(&self, id: i32) -> Result<&Interface> {
        self.gpus
            .iter()
            .find(|(gpu, _)| *gpu == id)
            .map(|(_, gpu)| gpu)
            .with_context(|| format!("ADLX reports no GPU with id {id}"))
    }
}

/// Query ADLX for the 3D settings of every AMD GPU. Reads only; no setting is written.
pub fn probe() -> Result<Probe> {
    let session = Session::open()?;
    let mut gpus = vec![];
    for (id, gpu) in &session.gpus {
        // SAFETY: `gpu` is an IADLXGPU.
        let gpu_table = unsafe { gpu.vtable::<GpuVt>() };
        let mut driver = DriverInfo::default();
        if let Ok(gpu2) = query(gpu, "IADLXGPU2") {
            // SAFETY: `gpu2` is an IADLXGPU2, whose layout extends IADLXGPU.
            let t = unsafe { gpu2.vtable::<Gpu2Vt>() };
            driver = DriverInfo {
                amd_software_version: get_text(&gpu2, t.software_version),
                amd_software_edition: get_text(&gpu2, t.edition),
                driver_version: get_text(&gpu2, t.driver_version),
                windows_driver_version: get_text(&gpu2, t.windows_driver_version),
            };
        }
        gpus.push(GpuReport {
            id: *id,
            name: get_text(gpu, gpu_table.name).unwrap_or_default(),
            driver_path: get_text(gpu, gpu_table.driver_path),
            driver,
            settings: session.settings(gpu),
        });
    }
    Ok(Probe {
        adlx: session.info.clone(),
        gpus,
    })
}

/// Reads and writes through ADLX. Every call opens its own session, so nothing is held between
/// calls and the host's telemetry keeps its own ADLX.
pub struct AdlxBackend;
impl RadeonBackend for AdlxBackend {
    fn read(&mut self, gpu: i32) -> Result<Vec<Setting>, BackendError> {
        let session = Session::open().map_err(|e| BackendError(format!("{e:#}")))?;
        let gpu = session
            .gpu(gpu)
            .map_err(|e| BackendError(format!("{e:#}")))?;
        Ok(session.settings(gpu))
    }
    fn write(&mut self, gpu: i32, id: &str, value: &Value) -> Result<(), BackendError> {
        let session = Session::open().map_err(|e| BackendError(format!("{e:#}")))?;
        let gpu = session
            .gpu(gpu)
            .map_err(|e| BackendError(format!("{e:#}")))?;
        write_setting(&session.ctx(gpu), id, value).map_err(BackendError)
    }
}

/// A setter's answer. ADLX_ALREADY_ENABLED (1) is what some setters say when the value is already
/// in effect; the caller's read-back still decides whether the write counts.
fn written(result: i32) -> Query<()> {
    code(if result == 1 { 0 } else { result })
}
fn set_bool(i: &Interface, set: SetBool, on: bool) -> Query<()> {
    // SAFETY: `set` is a SetEnabled slot of the live interface `i`.
    written(unsafe { set(i.0, u8::from(on)) })
}
fn set_int(i: &Interface, set: SetInt, value: i32) -> Query<()> {
    // SAFETY: `set` is a setter slot of the live interface `i` that takes one 32-bit value.
    written(unsafe { set(i.0, value) })
}
/// Why a setter could not be reached.
fn unreachable_reason(open: &Open) -> Fault {
    match (&open.obtained, &open.supported) {
        (Err(fault), _) | (_, Some(Err(fault))) => *fault,
        // IsSupported said false: ADLX_NOT_SUPPORTED.
        _ => Fault::Code(12),
    }
}
macro_rules! call {
    ($open:expr, $ty:ty, |$i:ident, $t:ident| $body:expr) => {{
        let open = $open;
        let (ready, table) = table!(open, $ty);
        match ready.zip(table) {
            Some(($i, $t)) => $body,
            None => Err(unreachable_reason(&open)),
        }
    }};
}

/// The only place a setter is called. Settings the host does not write have no arm.
fn write_setting(c: &Ctx, id: &str, value: &Value) -> Result<(), String> {
    let flag = || match value {
        Value::Bool(on) => Ok(*on),
        _ => Err(format!("{id} takes a toggle")),
    };
    let int = || match value {
        Value::Int(v) => i32::try_from(*v).map_err(|_| format!("{v} does not fit {id}")),
        _ => Err(format!("{id} takes an integer")),
    };
    let raw = || match value {
        Value::Choice(name) => enum_items(id)
            .and_then(|items| enum_raw(items, name))
            .ok_or_else(|| format!("{name} is not a value of {id}")),
        _ => Err(format!("{id} takes one of its options")),
    };
    let result: Query<()> = match id {
        "anti_lag.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anti_lag)),
                ToggleVt,
                |i, t| { set_bool(i, t.set_enabled, on) }
            )
        }
        "chill.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.with_gpu(|s| s.chill)), ChillVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        "chill.min_fps" => {
            let v = int()?;
            call!(Open::standard(c.with_gpu(|s| s.chill)), ChillVt, |i, t| {
                set_int(i, t.set_min_fps, v)
            })
        }
        "chill.max_fps" => {
            let v = int()?;
            call!(Open::standard(c.with_gpu(|s| s.chill)), ChillVt, |i, t| {
                set_int(i, t.set_max_fps, v)
            })
        }
        "boost.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.with_gpu(|s| s.boost)), RangedVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        "boost.min_resolution" => {
            let v = int()?;
            call!(Open::standard(c.with_gpu(|s| s.boost)), RangedVt, |i, t| {
                set_int(i, t.set_value, v)
            })
        }
        "image_sharpening.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.image_sharpening)),
                RangedVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "image_sharpening.sharpness" => {
            let v = int()?;
            call!(
                Open::standard(c.with_gpu(|s| s.image_sharpening)),
                RangedVt,
                |i, t| set_int(i, t.set_value, v)
            )
        }
        "enhanced_sync.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.enhanced_sync)),
                ToggleVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "vsync.mode" => {
            let v = raw()?;
            call!(Open::standard(c.with_gpu(|s| s.vsync)), VsyncVt, |i, t| {
                set_int(i, t.set_mode, v)
            })
        }
        "frame_rate_target.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.frame_rate_target)),
                RangedVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "frame_rate_target.fps" => {
            let v = int()?;
            call!(
                Open::standard(c.with_gpu(|s| s.frame_rate_target)),
                RangedVt,
                |i, t| set_int(i, t.set_value, v)
            )
        }
        "afmf.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.afmf()), ToggleVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        "afmf.search_mode" | "afmf.performance_mode" | "afmf.fast_motion_response" => {
            let v = raw()?;
            let parent = Open::standard(c.afmf());
            call!(
                Open::extension(&parent, "IADLX3DAMDFluidMotionFrames1"),
                Afmf1Vt,
                |i, t| match id {
                    "afmf.search_mode" => set_int(i, t.set_search_mode, v),
                    "afmf.performance_mode" => set_int(i, t.set_performance_mode, v),
                    _ => set_int(i, t.set_fast_motion, v),
                }
            )
        }
        "rsr.sharpness" => {
            let v = int()?;
            call!(Open::standard(c.rsr()), RsrVt, |i, t| {
                set_int(i, t.set_sharpness, v)
            })
        }
        "anti_aliasing.mode" | "anti_aliasing.level" | "anti_aliasing.method" => {
            let v = raw()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anti_aliasing)),
                AntiAliasingVt,
                |i, t| match id {
                    "anti_aliasing.mode" => set_int(i, t.set_mode, v),
                    "anti_aliasing.level" => set_int(i, t.set_level, v),
                    _ => set_int(i, t.set_method, v),
                }
            )
        }
        "morphological_aa.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.morphological_aa)),
                ToggleVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "anisotropic_filtering.enabled" => {
            let on = flag()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anisotropic_filtering)),
                AnisotropicVt,
                |i, t| set_bool(i, t.set_enabled, on)
            )
        }
        "anisotropic_filtering.level" => {
            let v = raw()?;
            call!(
                Open::standard(c.with_gpu(|s| s.anisotropic_filtering)),
                AnisotropicVt,
                |i, t| set_int(i, t.set_level, v)
            )
        }
        "tessellation.mode" | "tessellation.level" => {
            let v = raw()?;
            call!(
                Open::standard(c.with_gpu(|s| s.tessellation)),
                TessellationVt,
                |i, t| match id {
                    "tessellation.mode" => set_int(i, t.set_mode, v),
                    _ => set_int(i, t.set_level, v),
                }
            )
        }
        "fidelityfx_super_resolution.enabled" => {
            let on = flag()?;
            call!(Open::standard(c.fidelityfx(false)), ToggleVt, |i, t| {
                set_bool(i, t.set_enabled, on)
            })
        }
        _ => return Err(format!("{id} has no ADLX setter the host uses")),
    };
    result.map_err(|fault| format!("{id}: {}", fault.describe()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn vtable_offsets_match_the_public_c_interfaces() {
        let p = size_of::<usize>();
        assert_eq!(offset_of!(SystemVt, gpus), p);
        assert_eq!(offset_of!(SystemVt, services_3d), 7 * p);
        assert_eq!(offset_of!(ServicesVt, anti_lag), 3 * p);
        assert_eq!(offset_of!(ServicesVt, tessellation), 13 * p);
        assert_eq!(offset_of!(ServicesVt, rsr), 14 * p);
        assert_eq!(offset_of!(ServicesVt, reset_shader_cache), 15 * p);
        assert_eq!(offset_of!(ServicesVt, changed_handling), 16 * p);
        assert_eq!(offset_of!(Services1Vt, afmf), 17 * p);
        assert_eq!(offset_of!(Services2Vt, image_sharpen_desktop), 18 * p);
        assert_eq!(offset_of!(Services3Vt, fidelityfx_super_resolution), 19 * p);
        assert_eq!(offset_of!(Services3Vt, fidelityfx_frame_gen), 20 * p);
        assert_eq!(size_of::<Services3Vt>(), 21 * p);
        assert_eq!(offset_of!(GpuVt, name), 7 * p);
        assert_eq!(offset_of!(GpuVt, driver_path), 8 * p);
        assert_eq!(offset_of!(GpuVt, unique_id), 18 * p);
        assert_eq!(offset_of!(Gpu2Vt, edition), 30 * p);
        assert_eq!(offset_of!(Gpu2Vt, software_version), 31 * p);
        assert_eq!(offset_of!(Gpu2Vt, driver_version), 32 * p);
        assert_eq!(offset_of!(Gpu2Vt, windows_driver_version), 33 * p);
    }

    #[test]
    fn interface_vtable_offsets_match_the_public_c_interfaces() {
        let p = size_of::<usize>();
        assert_eq!(size_of::<ToggleVt>(), 6 * p);
        assert_eq!(offset_of!(AntiLag1Vt, level), 6 * p);
        assert_eq!(offset_of!(ChillVt, range), 5 * p);
        assert_eq!(offset_of!(ChillVt, max_fps), 7 * p);
        assert_eq!(size_of::<ChillVt>(), 11 * p);
        assert_eq!(offset_of!(RangedVt, value), 6 * p);
        assert_eq!(size_of::<RangedVt>(), 9 * p);
        assert_eq!(offset_of!(VsyncVt, mode), 5 * p);
        assert_eq!(size_of::<VsyncVt>(), 7 * p);
        assert_eq!(offset_of!(AntiAliasingVt, method), 6 * p);
        assert_eq!(size_of::<AntiAliasingVt>(), 10 * p);
        assert_eq!(offset_of!(AnisotropicVt, level), 5 * p);
        assert_eq!(size_of::<AnisotropicVt>(), 8 * p);
        assert_eq!(offset_of!(TessellationVt, level), 5 * p);
        assert_eq!(size_of::<TessellationVt>(), 8 * p);
        assert_eq!(offset_of!(RsrVt, range), 6 * p);
        assert_eq!(offset_of!(RsrVt, sharpness), 7 * p);
        assert_eq!(size_of::<RsrVt>(), 9 * p);
        assert_eq!(offset_of!(Afmf1Vt, algorithm_supported), 6 * p);
        assert_eq!(offset_of!(Afmf1Vt, fast_motion), 13 * p);
        assert_eq!(size_of::<Afmf1Vt>(), 15 * p);
        assert_eq!(offset_of!(FrameGenVt, supported), 5 * p);
        assert_eq!(offset_of!(FrameGenVt, enabled), 6 * p);
        assert_eq!(size_of::<FrameGenVt>(), 9 * p);
        assert_eq!(offset_of!(RatioOptionVt, ratio), 3 * p);
        assert_eq!(size_of::<RawRange>(), 12);
    }

    #[test]
    fn full_version_formats_as_four_parts() {
        assert_eq!(version_text(ADLX_VERSION), "1.0.5.30");
        assert_eq!(version_text((2 << 48) | (1 << 16) | 125), "2.0.1.125");
    }

    /// Needs an AMD GPU with the Radeon driver installed. Read-only.
    #[test]
    #[ignore = "needs an AMD GPU and the Radeon driver"]
    fn probe_reads_every_3d_setting_without_writing() {
        let probe = probe().expect("ADLX probe");
        assert!(!probe.gpus.is_empty(), "no AMD GPU reported");
        for gpu in &probe.gpus {
            assert!(gpu.settings.len() >= 20, "{} settings", gpu.settings.len());
            for setting in &gpu.settings {
                assert!(setting.adlx.contains_key("get_interface"), "{}", setting.id);
            }
        }
    }
}
