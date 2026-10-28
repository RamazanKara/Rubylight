//! Widens the HEVC picture order count AMF writes in four bits.
//!
//! A decoder derives each picture's POC from the low bits its slices carry,
//! taking the value nearest the last picture it decoded (H.265 8.3.1). AMF
//! writes four (`log2_max_pic_order_cnt_lsb_minus4 = 0`), so a client that
//! drops more than 8 frames before a long-term-reference repair derives a POC
//! 16 too low and looks for a reference it never had (rust/PERFORMANCE.md,
//! October 27). The host sees every picture and so knows every POC: it writes
//! the SPS with [`POC_LSB_BITS`] and each slice header's POC and long-term
//! references to match, which leaves the pictures as they were and lets a
//! repair reach a decoder up to [`ORDER_WINDOW`] frames after its last one.
use super::rbsp::{
    Reader, Replacement, Result, TRUNCATED, Unsupported, Writer, assemble, ceil_log2, splice,
    unescape, units,
};

/// Low bits of the POC in the rewritten stream: x265's default, and well
/// within the 16 the standard allows.
pub const POC_LSB_BITS: u32 = 8;
/// How many frames after its last picture a decoder still derives a POC.
pub const ORDER_WINDOW: u64 = 1 << (POC_LSB_BITS - 1);
/// Slice headers end long before this; entry points and extensions included.
const HEADER_LIMIT: usize = 4096;
/// The RBSP bytes read first: a header without many entry points fits.
const HEADER_FIRST: usize = 256;

#[derive(Clone, Default)]
struct Rps {
    /// DeltaPocS0 and UsedByCurrPicS0, nearest first.
    negative: Vec<(i32, bool)>,
    positive: Vec<(i32, bool)>,
}
impl Rps {
    fn len(&self) -> usize {
        self.negative.len() + self.positive.len()
    }
    fn deltas(&self) -> impl Iterator<Item = &(i32, bool)> {
        self.negative.iter().chain(&self.positive)
    }
}

struct Sps {
    lsb_bits: u32,
    separate_colour_plane: bool,
    chroma: bool,
    address_bits: u32,
    sao: bool,
    temporal_mvp: bool,
    long_term: bool,
    sets: Vec<Rps>,
}

struct Pps {
    sps: usize,
    dependent_slices: bool,
    output_flag: bool,
    extra_bits: usize,
    cabac_init: bool,
    l0_default: u32,
    l1_default: u32,
    chroma_offsets: bool,
    deblocking_override: bool,
    deblocking_disabled: bool,
    loop_filter_across_slices: bool,
    entry_points: bool,
    lists_modification: bool,
    header_extension: bool,
}

/// The POC and full long-term POCs of the picture being rewritten, for its
/// later slices.
struct Picture {
    poc: i64,
    long_term: Vec<(u32, i64)>,
}

/// Rewrites AMF's HEVC output in order, one buffer at a time.
#[derive(Default)]
pub struct PictureOrder {
    sps: Vec<Option<Sps>>,
    pps: Vec<Option<Pps>>,
    /// prevTid0Pic's POC and setOfPrevPocVals (H.265 8.3.1, 7.4.7.1).
    previous: Option<i64>,
    known: Vec<i64>,
    picture: Option<Picture>,
    after_end_of_sequence: bool,
}

impl PictureOrder {
    /// The buffer with its POC widened, `None` when nothing in it changes.
    /// An error leaves the stream unrewritable: what came before already was.
    pub fn rewrite(&mut self, bytes: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut edits = Vec::new();
        for (start, end) in units(bytes) {
            if let Some(nal) = self.unit(&bytes[start..end])? {
                edits.push(((start, end), nal));
            }
        }
        Ok((!edits.is_empty()).then(|| assemble(bytes, edits)))
    }

    fn unit(&mut self, nal: &[u8]) -> Result<Option<Replacement>> {
        if nal.len() < 3 {
            return Ok(None);
        }
        if nal[0] & 1 != 0 || nal[1] >> 3 != 0 {
            return Err(Unsupported("layered HEVC"));
        }
        match (nal[0] >> 1) & 63 {
            kind @ 0..=21 => match self.slice(kind, nal, HEADER_FIRST) {
                Err(TRUNCATED) if nal.len() > 2 + HEADER_FIRST => {
                    self.slice(kind, nal, HEADER_LIMIT)
                }
                result => result,
            },
            22..=31 => Err(Unsupported("reserved HEVC slice type")),
            33 => self.parameter_set(nal),
            34 => {
                let (rbsp, _) = unescape(&nal[2..], usize::MAX);
                let (id, pps) = picture_parameter_set(&rbsp)?;
                self.pps.resize_with(self.pps.len().max(id + 1), || None);
                self.pps[id] = Some(pps);
                Ok(None)
            }
            36 => {
                self.after_end_of_sequence = true;
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    fn parameter_set(&mut self, nal: &[u8]) -> Result<Option<Replacement>> {
        let (rbsp, _) = unescape(&nal[2..], usize::MAX);
        let (id, sps, field) = sequence_parameter_set(&rbsp)?;
        let widen = sps.lsb_bits < POC_LSB_BITS;
        self.sps.resize_with(self.sps.len().max(id + 1), || None);
        self.sps[id] = Some(sps);
        if !widen {
            return Ok(None);
        }
        let stop = rbsp
            .iter()
            .rposition(|&byte| byte != 0)
            .map(|index| index * 8 + 7 - rbsp[index].trailing_zeros() as usize)
            .ok_or(Unsupported("SPS without trailing bits"))?;
        let mut writer = Writer::default();
        writer.copy(&rbsp, 0, field.0);
        writer.ue(POC_LSB_BITS - 4);
        writer.copy(&rbsp, field.1, stop);
        let mut output = nal[..2].to_vec();
        super::rbsp::escape(&writer.finish(), &mut output);
        Ok(Some((output, nal.len())))
    }

    /// A slice's header rewritten from its first `limit` RBSP bytes. Nothing
    /// changes before the header has parsed, so a cut one can be read again.
    fn slice(&mut self, kind: u8, nal: &[u8], limit: usize) -> Result<Option<Replacement>> {
        let temporal_id = (nal[1] & 7)
            .checked_sub(1)
            .ok_or(Unsupported("NAL temporal id zero"))?;
        let (rbsp, from) = unescape(&nal[2..], limit);
        let mut reader = Reader::new(&rbsp);
        let first = reader.flag()?;
        if (16..=23).contains(&kind) {
            reader.skip(1)?;
        }
        let pps_id = reader.ue_max(63, "PPS id")? as usize;
        let pps = self
            .pps
            .get(pps_id)
            .and_then(Option::as_ref)
            .ok_or(Unsupported("slice before its PPS"))?;
        let sps = self
            .sps
            .get(pps.sps)
            .and_then(Option::as_ref)
            .ok_or(Unsupported("slice before its SPS"))?;
        if sps.lsb_bits >= POC_LSB_BITS {
            return Ok(None);
        }
        let mut dependent = false;
        if !first {
            if pps.dependent_slices {
                dependent = reader.flag()?;
            }
            reader.skip(sps.address_bits as usize)?;
        }
        if dependent {
            // Takes its header, POC included, from the segment before it.
            return Ok(None);
        }
        reader.skip(pps.extra_bits)?;
        let slice_type = reader.ue_max(2, "slice type")?;
        if pps.output_flag {
            reader.skip(1)?;
        }
        if sps.separate_colour_plane {
            reader.skip(2)?;
        }
        if kind == 19 || kind == 20 {
            if first {
                self.picture = Some(Picture {
                    poc: 0,
                    long_term: Vec::new(),
                });
                self.previous = Some(0);
                self.known = vec![0];
                self.after_end_of_sequence = false;
            }
            return Ok(None);
        }
        let poc_at = reader.at;
        let lsb = reader.bits(sps.lsb_bits)?;
        let short_term = if !reader.flag()? {
            short_term_set(&mut reader, sps.sets.len(), sps.sets.len(), &sps.sets)?
        } else {
            let bits = ceil_log2(sps.sets.len() as u32);
            let index = reader.bits(bits)? as usize;
            sps.sets
                .get(index)
                .cloned()
                .ok_or(Unsupported("short-term set index"))?
        };
        let long_term_at = reader.at;
        let mut long_term = Vec::new();
        if sps.long_term {
            for _ in 0..reader.ue_max(32, "long-term pictures")? {
                let lsb = reader.bits(sps.lsb_bits)?;
                let used = reader.flag()?;
                let cycle = if reader.flag()? {
                    Some(reader.ue_max(1 << 24, "MSB cycle")?)
                } else {
                    None
                };
                long_term.push((lsb, used, cycle));
            }
        }
        let long_term_end = reader.at;
        let used = short_term.deltas().filter(|(_, used)| *used).count()
            + long_term.iter().filter(|(_, used, _)| *used).count();
        let end = rest_of_header(&mut reader, sps, pps, slice_type, used as u32)?;

        let max = 1i64 << sps.lsb_bits;
        let picture = if first {
            let msb = if (16..=18).contains(&kind)
                || (kind == 21 && (self.previous.is_none() || self.after_end_of_sequence))
            {
                0
            } else {
                let previous = self
                    .previous
                    .ok_or(Unsupported("picture before any IRAP"))?;
                let (previous_lsb, lsb) = (previous.rem_euclid(max), i64::from(lsb));
                let msb = previous - previous_lsb;
                if lsb < previous_lsb && previous_lsb - lsb >= max / 2 {
                    msb + max
                } else if lsb > previous_lsb && lsb - previous_lsb > max / 2 {
                    msb - max
                } else {
                    msb
                }
            };
            let poc = msb + i64::from(lsb);
            let mut cycle = 0i64;
            let mut resolved = Vec::with_capacity(long_term.len());
            for (index, &(lt_lsb, _, delta)) in long_term.iter().enumerate() {
                let full = match delta {
                    Some(delta) => {
                        cycle = if index == 0 {
                            i64::from(delta)
                        } else {
                            cycle + i64::from(delta)
                        };
                        i64::from(lt_lsb) + poc - cycle * max - i64::from(lsb)
                    }
                    None => {
                        let mut matches = self
                            .known
                            .iter()
                            .filter(|&&known| known.rem_euclid(max) == i64::from(lt_lsb));
                        match (matches.next(), matches.next()) {
                            (Some(&known), None) => known,
                            _ => return Err(Unsupported("long-term reference not unique")),
                        }
                    }
                };
                resolved.push((lt_lsb, full));
            }
            let mut known: Vec<i64> = std::iter::once(poc)
                .chain(
                    short_term
                        .deltas()
                        .map(|(delta, _)| poc + i64::from(*delta)),
                )
                .chain(resolved.iter().map(|(_, full)| *full))
                .collect();
            let sub_layer_non_reference = kind <= 14 && kind.is_multiple_of(2);
            if temporal_id == 0 && !sub_layer_non_reference && !(6..=9).contains(&kind) {
                self.previous = Some(poc);
                std::mem::swap(&mut self.known, &mut known);
            } else {
                self.known.push(poc);
            }
            self.after_end_of_sequence = false;
            &*self.picture.insert(Picture {
                poc,
                long_term: resolved,
            })
        } else {
            let picture = self
                .picture
                .as_ref()
                .ok_or(Unsupported("slice without its picture's first slice"))?;
            if picture.poc.rem_euclid(max) != i64::from(lsb)
                || picture.long_term.len() != long_term.len()
                || picture
                    .long_term
                    .iter()
                    .zip(&long_term)
                    .any(|((first, _), (this, _, _))| first != this)
            {
                return Err(Unsupported("slices of one picture disagree"));
            }
            picture
        };

        let wide = 1i64 << POC_LSB_BITS;
        let mut writer = Writer::default();
        writer.copy(&rbsp, 0, poc_at);
        writer.bits(POC_LSB_BITS, picture.poc.rem_euclid(wide) as u32);
        writer.copy(&rbsp, poc_at + sps.lsb_bits as usize, long_term_at);
        if sps.long_term {
            writer.ue(long_term.len() as u32);
            // Every entry names its MSB, so the reference resolves from the
            // POC alone, whatever else a lossy client still holds.
            let mut previous = 0;
            for ((_, full), (_, used, _)) in picture.long_term.iter().zip(&long_term) {
                writer.bits(POC_LSB_BITS, full.rem_euclid(wide) as u32);
                writer.flag(*used);
                writer.flag(true);
                let cycle = picture.poc.div_euclid(wide) - full.div_euclid(wide);
                let delta = u32::try_from(cycle - previous)
                    .map_err(|_| Unsupported("long-term references out of order"))?;
                writer.ue(delta);
                previous = cycle;
            }
        }
        writer.copy(&rbsp, long_term_end, end);
        let header = writer.finish();
        let length = end / 8 + 1;
        Ok(Some(splice(nal, 2, &header, &from, length)))
    }
}

fn profile_tier_level(reader: &mut Reader, sub_layers: u32) -> Result<u32> {
    let profile = reader.bits(8)? & 31;
    reader.skip(32 + 4 + 43 + 1 + 8)?;
    let mut present = Vec::new();
    for _ in 0..sub_layers {
        present.push((reader.flag()?, reader.flag()?));
    }
    if sub_layers > 0 {
        reader.skip(2 * (8 - sub_layers as usize))?;
    }
    for (profile, level) in present {
        reader.skip(if profile { 88 } else { 0 } + if level { 8 } else { 0 })?;
    }
    Ok(profile)
}

fn scaling_list_data(reader: &mut Reader) -> Result<()> {
    for size in 0..4 {
        let mut matrix = 0;
        while matrix < 6 {
            if !reader.flag()? {
                reader.ue()?;
            } else {
                if size > 1 {
                    reader.se()?;
                }
                for _ in 0..64.min(1 << (4 + (size << 1))) {
                    reader.se()?;
                }
            }
            matrix += if size == 3 { 3 } else { 1 };
        }
    }
    Ok(())
}

/// st_ref_pic_set(`index`) of `count` in the SPS, and its derivation (H.265
/// 7.3.7, 7.4.8). The slice header's own set has `index == count`.
fn short_term_set(reader: &mut Reader, index: usize, count: usize, sets: &[Rps]) -> Result<Rps> {
    let parsing_slice = index == count;
    if index != 0 && reader.flag()? {
        let delta_index = if parsing_slice {
            reader.ue_max(63, "RPS delta index")? as usize + 1
        } else {
            1
        };
        let reference = index
            .checked_sub(delta_index)
            .and_then(|index| sets.get(index))
            .ok_or(Unsupported("RPS reference"))?;
        let sign = reader.flag()?;
        let magnitude = reader.ue_max(1 << 15, "RPS delta")? as i32 + 1;
        let delta = if sign { -magnitude } else { magnitude };
        let mut used = Vec::with_capacity(reference.len() + 1);
        for _ in 0..=reference.len() {
            let current = reader.flag()?;
            used.push((current, current || reader.flag()?));
        }
        let negatives = reference.negative.len();
        let mut set = Rps::default();
        for (index, &(base, _)) in reference.positive.iter().enumerate().rev() {
            let (current, kept) = used[negatives + index];
            if base + delta < 0 && kept {
                set.negative.push((base + delta, current));
            }
        }
        let (current, kept) = used[reference.len()];
        if delta < 0 && kept {
            set.negative.push((delta, current));
        }
        for (index, &(base, _)) in reference.negative.iter().enumerate() {
            let (current, kept) = used[index];
            if base + delta < 0 && kept {
                set.negative.push((base + delta, current));
            }
        }
        for (index, &(base, _)) in reference.negative.iter().enumerate().rev() {
            let (current, kept) = used[index];
            if base + delta > 0 && kept {
                set.positive.push((base + delta, current));
            }
        }
        if delta > 0 && kept {
            set.positive.push((delta, current));
        }
        for (index, &(base, _)) in reference.positive.iter().enumerate() {
            let (current, kept) = used[negatives + index];
            if base + delta > 0 && kept {
                set.positive.push((base + delta, current));
            }
        }
        if set.len() > 16 {
            return Err(Unsupported("RPS over 16 pictures"));
        }
        return Ok(set);
    }
    let negatives = reader.ue_max(16, "RPS size")?;
    let positives = reader.ue_max(16, "RPS size")?;
    let mut set = Rps::default();
    let mut poc = 0i32;
    for _ in 0..negatives {
        poc -= reader.ue_max(1 << 15, "RPS delta")? as i32 + 1;
        set.negative.push((poc, reader.flag()?));
    }
    poc = 0;
    for _ in 0..positives {
        poc += reader.ue_max(1 << 15, "RPS delta")? as i32 + 1;
        set.positive.push((poc, reader.flag()?));
    }
    Ok(set)
}

/// The SPS, its id and the bit range of log2_max_pic_order_cnt_lsb_minus4.
fn sequence_parameter_set(rbsp: &[u8]) -> Result<(usize, Sps, (usize, usize))> {
    let mut reader = Reader::new(rbsp);
    reader.skip(4)?;
    let sub_layers = reader.bits(3)?;
    if sub_layers > 6 {
        return Err(Unsupported("SPS sub-layers"));
    }
    reader.skip(1)?;
    // Main, Main 10 and Main Still Picture: no range or screen content
    // extension adds slice header syntax.
    if !(1..=3).contains(&profile_tier_level(&mut reader, sub_layers)?) {
        return Err(Unsupported("HEVC profile other than Main or Main 10"));
    }
    let id = reader.ue_max(15, "SPS id")? as usize;
    let chroma_format = reader.ue_max(3, "chroma format")?;
    let separate_colour_plane = chroma_format == 3 && reader.flag()?;
    let width = reader.ue()?;
    let height = reader.ue()?;
    if reader.flag()? {
        for _ in 0..4 {
            reader.ue()?;
        }
    }
    reader.ue()?;
    reader.ue()?;
    let field_start = reader.at;
    let lsb_bits = reader.ue_max(12, "POC LSB length")? + 4;
    let field = (field_start, reader.at);
    let all_sub_layers = reader.flag()?;
    for _ in if all_sub_layers { 0 } else { sub_layers }..=sub_layers {
        reader.ue()?;
        reader.ue()?;
        reader.ue()?;
    }
    let min_block = reader.ue_max(3, "coding block size")? + 3;
    let ctb = min_block + reader.ue_max(3, "coding block size")?;
    if ctb > 6 {
        return Err(Unsupported("CTB over 64"));
    }
    for _ in 0..4 {
        reader.ue()?;
    }
    if reader.flag()? && reader.flag()? {
        scaling_list_data(&mut reader)?;
    }
    reader.skip(1)?;
    let sao = reader.flag()?;
    if reader.flag()? {
        reader.skip(8)?;
        reader.ue()?;
        reader.ue()?;
        reader.skip(1)?;
    }
    let count = reader.ue_max(64, "short-term sets")? as usize;
    let mut sets = Vec::with_capacity(count);
    for index in 0..count {
        let set = short_term_set(&mut reader, index, count, &sets)?;
        sets.push(set);
    }
    let long_term = reader.flag()?;
    if long_term && reader.ue()? > 0 {
        return Err(Unsupported("long-term pictures listed in the SPS"));
    }
    let temporal_mvp = reader.flag()?;
    let size = |samples: u32| samples.div_ceil(1 << ctb);
    let ctbs = size(width)
        .checked_mul(size(height))
        .ok_or(Unsupported("picture size"))?;
    let sps = Sps {
        lsb_bits,
        separate_colour_plane,
        chroma: chroma_format != 0 && !separate_colour_plane,
        address_bits: ceil_log2(ctbs),
        sao,
        temporal_mvp,
        long_term,
        sets,
    };
    Ok((id, sps, field))
}

fn picture_parameter_set(rbsp: &[u8]) -> Result<(usize, Pps)> {
    let mut reader = Reader::new(rbsp);
    let id = reader.ue_max(63, "PPS id")? as usize;
    let sps = reader.ue_max(15, "SPS id")? as usize;
    let dependent_slices = reader.flag()?;
    let output_flag = reader.flag()?;
    let extra_bits = reader.bits(3)? as usize;
    reader.skip(1)?;
    let cabac_init = reader.flag()?;
    let l0_default = reader.ue_max(14, "reference count")?;
    let l1_default = reader.ue_max(14, "reference count")?;
    reader.se()?;
    reader.skip(2)?;
    if reader.flag()? {
        reader.ue()?;
    }
    reader.se()?;
    reader.se()?;
    let chroma_offsets = reader.flag()?;
    let (weighted, weighted_bipred) = (reader.flag()?, reader.flag()?);
    if weighted || weighted_bipred {
        return Err(Unsupported("weighted prediction"));
    }
    reader.skip(1)?;
    let tiles = reader.flag()?;
    let wavefronts = reader.flag()?;
    if tiles {
        let columns = reader.ue_max(64, "tile columns")?;
        let rows = reader.ue_max(64, "tile rows")?;
        if !reader.flag()? {
            for _ in 0..columns + rows {
                reader.ue()?;
            }
        }
        reader.skip(1)?;
    }
    let loop_filter_across_slices = reader.flag()?;
    let (mut deblocking_override, mut deblocking_disabled) = (false, false);
    if reader.flag()? {
        deblocking_override = reader.flag()?;
        deblocking_disabled = reader.flag()?;
        if !deblocking_disabled {
            reader.se()?;
            reader.se()?;
        }
    }
    if reader.flag()? {
        scaling_list_data(&mut reader)?;
    }
    let lists_modification = reader.flag()?;
    reader.ue()?;
    let header_extension = reader.flag()?;
    if reader.flag()? {
        let (range, _multilayer, _three_d, screen_content) = (
            reader.flag()?,
            reader.flag()?,
            reader.flag()?,
            reader.flag()?,
        );
        if range || screen_content {
            return Err(Unsupported("PPS range or screen content extension"));
        }
    }
    let pps = Pps {
        sps,
        dependent_slices,
        output_flag,
        extra_bits,
        cabac_init,
        l0_default,
        l1_default,
        chroma_offsets,
        deblocking_override,
        deblocking_disabled,
        loop_filter_across_slices,
        entry_points: tiles || wavefronts,
        lists_modification,
        header_extension,
    };
    Ok((id, pps))
}

/// Parses the slice header after its reference picture sets, through the
/// alignment bit, and returns where that bit starts (H.265 7.3.6.1).
fn rest_of_header(
    reader: &mut Reader,
    sps: &Sps,
    pps: &Pps,
    slice_type: u32,
    used: u32,
) -> Result<usize> {
    let temporal_mvp = sps.temporal_mvp && reader.flag()?;
    let (mut sao_luma, mut sao_chroma) = (false, false);
    if sps.sao {
        sao_luma = reader.flag()?;
        if sps.chroma {
            sao_chroma = reader.flag()?;
        }
    }
    let bidirectional = slice_type == 0;
    if slice_type < 2 {
        let (mut l0, mut l1) = (pps.l0_default, pps.l1_default);
        if reader.flag()? {
            l0 = reader.ue_max(14, "reference count")?;
            if bidirectional {
                l1 = reader.ue_max(14, "reference count")?;
            }
        }
        if pps.lists_modification && used > 1 {
            let bits = ceil_log2(used) as usize;
            if reader.flag()? {
                reader.skip(bits * (l0 as usize + 1))?;
            }
            if bidirectional && reader.flag()? {
                reader.skip(bits * (l1 as usize + 1))?;
            }
        }
        if bidirectional {
            reader.skip(1)?;
        }
        if pps.cabac_init {
            reader.skip(1)?;
        }
        if temporal_mvp {
            let from_l0 = !bidirectional || reader.flag()?;
            if (from_l0 && l0 > 0) || (!from_l0 && l1 > 0) {
                reader.ue()?;
            }
        }
        reader.ue()?;
    }
    reader.se()?;
    if pps.chroma_offsets {
        reader.se()?;
        reader.se()?;
    }
    let mut deblocking_disabled = pps.deblocking_disabled;
    if pps.deblocking_override && reader.flag()? {
        deblocking_disabled = reader.flag()?;
        if !deblocking_disabled {
            reader.se()?;
            reader.se()?;
        }
    }
    if pps.loop_filter_across_slices && (sao_luma || sao_chroma || !deblocking_disabled) {
        reader.skip(1)?;
    }
    if pps.entry_points {
        let offsets = reader.ue_max(1 << 16, "entry points")? as usize;
        if offsets > 0 {
            let bits = reader.ue_max(31, "entry point length")? as usize + 1;
            reader.skip(offsets * bits)?;
        }
    }
    if pps.header_extension {
        let length = reader.ue_max(256, "slice header extension")? as usize;
        reader.skip(length * 8)?;
    }
    let end = reader.at;
    if !reader.flag()? {
        return Err(Unsupported("slice header alignment"));
    }
    while !reader.at.is_multiple_of(8) {
        if reader.flag()? {
            return Err(Unsupported("slice header alignment"));
        }
    }
    Ok(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AMF's HEVC from the RX 7900 XT run that froze at gap 8: one slice a
    /// frame for frames 0 to 259, each cut after its header. Frame 249 is the
    /// repair of 241..=248 and names frame 239 as its long-term reference.
    const STREAM: &[u8] = include_bytes!("fixtures/amf-hevc-ltr.bin");

    /// Per picture: its POC LSB width and value, and its long-term entries
    /// (LSB, used, MSB cycle), read back with the parsers above.
    type Header = (u32, u32, Vec<(u32, bool, Option<u32>)>);
    fn headers(stream: &[u8]) -> Vec<Header> {
        let (mut sps, mut pps, mut pictures) = (None, None, Vec::new());
        for (start, end) in units(stream) {
            let nal = &stream[start..end];
            let kind = (nal[0] >> 1) & 63;
            let (rbsp, _) = unescape(&nal[2..], usize::MAX);
            match kind {
                33 => sps = Some(sequence_parameter_set(&rbsp).unwrap().1),
                34 => pps = Some(picture_parameter_set(&rbsp).unwrap().1),
                0..=21 => {
                    let (sps, pps) = (sps.as_ref().unwrap(), pps.as_ref().unwrap());
                    let mut reader = Reader::new(&rbsp);
                    assert!(reader.flag().unwrap(), "one slice a frame");
                    if (16..=23).contains(&kind) {
                        reader.skip(1).unwrap();
                    }
                    reader.ue().unwrap();
                    reader.skip(pps.extra_bits).unwrap();
                    reader.ue().unwrap();
                    if kind == 19 || kind == 20 {
                        pictures.push((sps.lsb_bits, 0, Vec::new()));
                        continue;
                    }
                    let lsb = reader.bits(sps.lsb_bits).unwrap();
                    if reader.flag().unwrap() {
                        reader.bits(ceil_log2(sps.sets.len() as u32)).unwrap();
                    } else {
                        let count = sps.sets.len();
                        short_term_set(&mut reader, count, count, &sps.sets).unwrap();
                    }
                    let mut long_term = Vec::new();
                    for _ in 0..reader.ue().unwrap() {
                        let lsb = reader.bits(sps.lsb_bits).unwrap();
                        let used = reader.flag().unwrap();
                        let cycle = reader.flag().unwrap().then(|| reader.ue().unwrap());
                        long_term.push((lsb, used, cycle));
                    }
                    pictures.push((sps.lsb_bits, lsb, long_term));
                }
                _ => {}
            }
        }
        pictures
    }

    /// The POC and full long-term POCs that a decoder given only the
    /// pictures `shown` derives for each (H.265 8.3.1, 8.3.2); a long-term
    /// entry without its MSB is `None`.
    fn derived(
        headers: &[Header],
        shown: impl Fn(usize) -> bool,
    ) -> Vec<(usize, i64, Vec<Option<i64>>)> {
        let mut previous = 0i64;
        let mut pictures = Vec::new();
        for (index, (bits, lsb, long_term)) in headers.iter().enumerate() {
            if !shown(index) {
                continue;
            }
            let max = 1i64 << bits;
            let (previous_lsb, lsb) = (previous.rem_euclid(max), i64::from(*lsb));
            let msb = previous - previous_lsb;
            let poc = if index == 0 {
                0
            } else if lsb < previous_lsb && previous_lsb - lsb >= max / 2 {
                msb + max + lsb
            } else if lsb > previous_lsb && lsb - previous_lsb > max / 2 {
                msb - max + lsb
            } else {
                msb + lsb
            };
            let mut cycle = 0;
            let references = long_term
                .iter()
                .map(|&(lt, _, delta)| {
                    delta.map(|delta| {
                        cycle += i64::from(delta);
                        poc - cycle * max - poc.rem_euclid(max) + i64::from(lt)
                    })
                })
                .collect();
            pictures.push((index, poc, references));
            previous = poc;
        }
        pictures
    }

    #[test]
    fn widened_pocs_let_a_decoder_that_skipped_frames_order_the_repair() {
        assert_eq!(ORDER_WINDOW, 128);
        let original = headers(STREAM);
        assert_eq!(original.len(), 260);
        assert!(original.iter().all(|(bits, ..)| *bits == 4));
        let rewritten = PictureOrder::default().rewrite(STREAM).unwrap().unwrap();
        let wide = headers(&rewritten);
        assert_eq!(wide.len(), 260);
        assert!(wide.iter().all(|(bits, ..)| *bits == POC_LSB_BITS));
        // Every picture keeps its order and its long-term references, now
        // named by MSB: the IDR or an anchor marked every fourth frame.
        for (index, poc, references) in derived(&wide, |_| true) {
            assert_eq!(poc, index as i64);
            assert_eq!(references.len(), original[index].2.len(), "frame {index}");
            for reference in references {
                let reference = reference.unwrap();
                assert!(
                    reference < poc && (reference == 0 || reference % 4 == 3),
                    "frame {index}: {reference}"
                );
            }
        }
        assert_eq!(wide[249].2[0], (239, true, Some(0)));
        // The client dropped 241..=248. From AMF's four bits it derives POC
        // 233 for the repair; from eight, 249, and the anchor it still has.
        let lossy = |index: usize| !(241..=248).contains(&index);
        let repair = |headers: &[Header]| {
            derived(headers, lossy)
                .into_iter()
                .find(|(index, ..)| *index == 249)
                .unwrap()
        };
        assert_eq!(repair(&original).1, 233);
        assert_eq!(repair(&wide).1, 249);
        assert_eq!(repair(&wide).2[0], Some(239));
        assert!(
            derived(&wide, lossy)
                .iter()
                .all(|(index, poc, _)| *poc == *index as i64)
        );
        // The window: 128 frames after the last one shown still order, 129
        // land 256 too low.
        for (last, ordered) in [(101, true), (100, false)] {
            let pictures = derived(&wide, |index| index <= last || index >= 229);
            let after = pictures.iter().find(|(index, ..)| *index == 229).unwrap();
            assert_eq!(after.1 == 229, ordered, "last shown {last}");
        }
        // Frame by frame, as AMF returns them, gives the same stream, and a
        // widened stream needs nothing more.
        let units = units(STREAM);
        let mut cuts: Vec<usize> = units[4..].iter().map(|unit| unit.0 - 4).collect();
        cuts.insert(0, 0);
        cuts.push(STREAM.len());
        let mut order = PictureOrder::default();
        let mut frames = Vec::new();
        for cut in cuts.windows(2) {
            let frame = &STREAM[cut[0]..cut[1]];
            frames.extend(order.rewrite(frame).unwrap().unwrap());
        }
        assert_eq!(frames, rewritten);
        assert_eq!(PictureOrder::default().rewrite(&rewritten), Ok(None));
    }

    /// A slice like `template`, one of `STREAM`'s P slices, for picture
    /// `poc` naming the long-term pictures `long_term` (POCs, newest first,
    /// the first used) as AMF writes them: four LSB bits and every MSB.
    fn amf_slice(template: &[u8], poc: i64, long_term: &[i64]) -> Vec<u8> {
        let units = units(STREAM);
        let rbsp = |unit: (usize, usize)| unescape(&STREAM[unit.0 + 2..unit.1], usize::MAX).0;
        let sps = sequence_parameter_set(&rbsp(units[1])).unwrap().1;
        let pps = picture_parameter_set(&rbsp(units[2])).unwrap().1;
        assert!(sps.lsb_bits == 4 && sps.long_term && !pps.output_flag);
        let (rbsp, _) = unescape(&template[2..], usize::MAX);
        let mut reader = Reader::new(&rbsp);
        assert!(reader.flag().unwrap(), "one slice a frame");
        reader.ue().unwrap();
        reader.skip(pps.extra_bits).unwrap();
        let slice_type = reader.ue().unwrap();
        let poc_at = reader.at;
        reader.skip(4).unwrap();
        let short_term = if reader.flag().unwrap() {
            sps.sets[reader.bits(ceil_log2(sps.sets.len() as u32)).unwrap() as usize].clone()
        } else {
            let count = sps.sets.len();
            short_term_set(&mut reader, count, count, &sps.sets).unwrap()
        };
        let long_term_at = reader.at;
        let mut used = short_term.deltas().filter(|(_, used)| *used).count() as u32;
        for _ in 0..reader.ue().unwrap() {
            reader.skip(4).unwrap();
            used += u32::from(reader.flag().unwrap());
            if reader.flag().unwrap() {
                reader.ue().unwrap();
            }
        }
        let long_term_end = reader.at;
        // One long-term picture is used, in the template as in its copies.
        let end = rest_of_header(&mut reader, &sps, &pps, slice_type, used).unwrap();

        let mut writer = Writer::default();
        writer.copy(&rbsp, 0, poc_at);
        writer.bits(4, poc.rem_euclid(16) as u32);
        writer.copy(&rbsp, poc_at + 4, long_term_at);
        writer.ue(long_term.len() as u32);
        let mut previous = 0;
        for (index, full) in long_term.iter().enumerate() {
            writer.bits(4, full.rem_euclid(16) as u32);
            writer.flag(index == 0);
            writer.flag(true);
            let cycle = poc.div_euclid(16) - full.div_euclid(16);
            writer.ue((cycle - previous) as u32);
            previous = cycle;
        }
        writer.copy(&rbsp, long_term_end, end);
        let mut nal = vec![0, 0, 0, 1, template[0], template[1]];
        super::super::rbsp::escape(&writer.finish(), &mut nal);
        nal
    }

    const REPAIR: u64 = 801;
    /// AMF's stream for wire frames 1..=840 as `References` plans it, POC
    /// the wire frame less one, with the loss of `first_lost..REPAIR`
    /// reported before frame `REPAIR`: the stream up to the end, or to the
    /// repair the host turned into a keyframe, each picture's long-term
    /// POCs, and whether it repaired.
    fn amf_stream(first_lost: u64) -> (Vec<u8>, Vec<Vec<i64>>, bool) {
        use crate::ltr::{Counter, References};
        let units = units(STREAM);
        let template = &STREAM[units[7].0..units[7].1];
        // VPS, SPS, PPS and the IDR, wire frame 1.
        let mut stream = STREAM[..units[3].1].to_vec();
        let mut pictures = vec![Vec::new()];
        let mut references = References::new(4, Counter::PictureOrder);
        let mut slots = [None; 4];
        for frame in 1..=840u64 {
            if frame == REPAIR && !references.invalidate(first_lost, REPAIR - 1) {
                return (stream, pictures, false);
            }
            let plan = references.plan(frame, frame == 1);
            if let Some(slot) = plan.reference {
                // A repair names only its reference, which is all it keeps.
                slots = std::array::from_fn(|index| slots[index].filter(|_| index == slot));
            }
            if frame > 1 {
                let mut long_term: Vec<i64> =
                    slots.iter().flatten().map(|&f| f as i64 - 1).collect();
                long_term.sort_unstable_by(|a, b| b.cmp(a));
                stream.extend(amf_slice(template, frame as i64 - 1, &long_term));
                pictures.push(long_term);
            }
            references.accepted(frame, &plan);
            if let Some(slot) = plan.mark {
                slots[slot] = Some(frame);
            }
        }
        (stream, pictures, true)
    }

    #[test]
    fn a_long_stream_keeps_its_order_across_wraps_and_repairs_up_to_the_window() {
        // The client last showed wire frame 673 (POC 672), 128 before the
        // repair (POC 800, after the third wrap of eight bits at 768).
        let (stream, pictures, repaired) = amf_stream(674);
        assert!(repaired);
        assert_eq!(pictures.len(), 840);
        let rewritten = PictureOrder::default().rewrite(&stream).unwrap().unwrap();
        let wide = headers(&rewritten);
        assert_eq!(wide.len(), 840);
        assert!(wide.iter().all(|(bits, ..)| *bits == POC_LSB_BITS));
        for (index, poc, references) in derived(&wide, |_| true) {
            assert_eq!(poc, index as i64);
            let expected: Vec<_> = pictures[index].iter().map(|&poc| Some(poc)).collect();
            assert_eq!(references, expected, "frame {index}");
        }
        // Long-term pictures on both sides of the wrap at 768, and the IDR
        // three wraps back.
        assert_eq!(
            wide[772].2,
            [
                (3, true, Some(0)),
                (255, false, Some(1)),
                (251, false, Some(0)),
                (0, false, Some(2)),
            ]
        );
        assert_eq!(wide[800].2, [(0, true, Some(3))]);
        // 128 frames after its last picture the client orders the repair;
        // one more and it would derive POC 544, so the host sends a keyframe.
        for (last, ordered) in [(672, true), (671, false)] {
            let pictures = derived(&wide, |index| index <= last || index >= 800);
            let repair = pictures.iter().find(|(index, ..)| *index == 800).unwrap();
            assert_eq!(repair.1 == 800, ordered, "last shown {last}");
            if ordered {
                assert_eq!(repair.2, [Some(0)]);
                assert!(pictures.iter().all(|(index, poc, _)| *poc == *index as i64));
            } else {
                assert_eq!(repair.1, 544);
            }
        }
        let (_, _, repaired) = amf_stream(673);
        assert!(
            !repaired,
            "129 frames after the last picture shown is a keyframe"
        );
        // Frame by frame, as AMF returns them, gives the same stream.
        let units = super::units(&stream);
        let mut order = PictureOrder::default();
        let mut frames = order.rewrite(&stream[..units[4].0 - 4]).unwrap().unwrap();
        for unit in units[4..].windows(2) {
            frames.extend(
                order
                    .rewrite(&stream[unit[0].0 - 4..unit[1].0 - 4])
                    .unwrap()
                    .unwrap(),
            );
        }
        let last = units.last().unwrap();
        frames.extend(order.rewrite(&stream[last.0 - 4..]).unwrap().unwrap());
        assert_eq!(frames, rewritten);
    }

    /// Deterministic bytes for the mutation test (xorshift).
    fn random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn malformed_hevc_is_an_error_and_never_a_panic() {
        let units = units(STREAM);
        let (sps, slice) = (units[1], units[23]);
        let order = |bytes: &[u8]| PictureOrder::default().rewrite(bytes);
        // A slice before its parameter sets, a layered unit, and the SPS or
        // a P slice's header cut short.
        assert_eq!(
            order(&STREAM[slice.0 - 4..]),
            Err(Unsupported("slice before its PPS"))
        );
        let mut layered = STREAM.to_vec();
        layered[sps.0 + 1] |= 8;
        assert_eq!(order(&layered), Err(Unsupported("layered HEVC")));
        for cut in 3..14 {
            let mut bytes = STREAM[..sps.0 + cut].to_vec();
            bytes.extend_from_slice(&STREAM[sps.1..]);
            assert!(order(&bytes).is_err(), "SPS cut to {cut}");
        }
        for cut in 3..6 {
            let mut bytes = STREAM[..slice.0 + cut].to_vec();
            bytes.extend_from_slice(&STREAM[slice.1..]);
            assert!(order(&bytes).is_err(), "slice cut to {cut}");
        }
        // Flipped bits and cuts anywhere in the parameter sets and the first
        // frames: an answer either way, and a rewrite keeps every unit.
        let mut state = 0x9E37_79B9_7F4A_7C15;
        let mut rewrites = 0;
        for _ in 0..3000 {
            let mut bytes = STREAM[..units[12].1].to_vec();
            for _ in 0..1 + random(&mut state) % 4 {
                let at = random(&mut state) as usize % bytes.len();
                bytes[at] ^= 1 << (random(&mut state) % 8);
            }
            bytes.truncate(bytes.len() - random(&mut state) as usize % 64);
            if let Ok(Some(rewritten)) = order(&bytes) {
                assert_eq!(super::units(&rewritten).len(), super::units(&bytes).len());
                rewrites += 1;
            }
        }
        assert!(rewrites > 100, "{rewrites}");
    }
}
