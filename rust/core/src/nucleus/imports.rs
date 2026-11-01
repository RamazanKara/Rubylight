//! The DLLs a Windows program imports, read from the import tables of its
//! file: only its headers, the tables and the names they point to are read,
//! each within bounds, so a game's file of any size or shape is read safely.

use std::io::{self, Read, Seek, SeekFrom};

/// The most DLLs one table names that are read: a game names a few dozen.
const MOST_DLLS: u64 = 1024;
/// The most sections a program has (the PE format's limit).
const MOST_SECTIONS: u16 = 96;
/// The longest DLL name read.
const NAME_LIMIT: u64 = 260;
/// The DLLs that are DirectInput.
const DIRECT_INPUT: [&str; 2] = ["dinput8.dll", "dinput.dll"];

fn invalid(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("not a Windows program: {what}"),
    )
}

fn read_at(file: &mut (impl Read + Seek), offset: u64, len: usize) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; len];
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            invalid("it ends inside its headers or import tables")
        } else {
            error
        }
    })?;
    Ok(bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// A section: where its bytes sit in memory and in the file.
struct Section {
    address: u64,
    size: u64,
    raw_size: u64,
    raw_offset: u64,
}

/// Where the image's address `rva` sits in the file, if a section's file
/// bytes hold it.
fn file_offset(sections: &[Section], rva: u64) -> Option<u64> {
    sections.iter().find_map(|section| {
        let into = rva.checked_sub(section.address)?;
        // A section's memory beyond its file bytes is zeros, and its file
        // bytes beyond its memory are padding.
        let mapped = if section.size == 0 {
            section.raw_size
        } else {
            section.size
        };
        (into < mapped && into < section.raw_size).then(|| section.raw_offset + into)
    })
}

fn name_at(file: &mut (impl Read + Seek), sections: &[Section], rva: u64) -> io::Result<String> {
    let offset =
        file_offset(sections, rva).ok_or_else(|| invalid("a DLL name outside its file"))?;
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.take(NAME_LIMIT).read_to_end(&mut bytes)?;
    let end = bytes
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| invalid("a DLL name without an end"))?;
    let name = &bytes[..end];
    if !name.is_ascii() {
        return Err(invalid("a DLL name that is not ASCII"));
    }
    Ok(String::from_utf8_lossy(name).to_ascii_lowercase())
}

/// The DLLs `program` imports, loaded with it or delay-loaded, in lowercase
/// as its import tables name them.
pub fn imported_dlls(program: &mut (impl Read + Seek)) -> io::Result<Vec<String>> {
    let dos = read_at(program, 0, 64)?;
    if !dos.starts_with(b"MZ") {
        return Err(invalid("no MZ header"));
    }
    let nt = u64::from(u32_at(&dos, 0x3c).unwrap_or_default());
    let coff = read_at(program, nt, 24)?;
    if !coff.starts_with(b"PE\0\0") {
        return Err(invalid("no PE header"));
    }
    let section_count = u16_at(&coff, 6).unwrap_or_default();
    if section_count > MOST_SECTIONS {
        return Err(invalid("too many sections"));
    }
    let optional_size = u16_at(&coff, 20).unwrap_or_default();
    let optional = read_at(program, nt + 24, usize::from(optional_size))?;
    // PE32 or PE32+: where the image base, the directory count and the
    // directories are.
    let (image_base, count_at, directories_at) = match u16_at(&optional, 0) {
        Some(0x10b) => (u32_at(&optional, 28).map(u64::from), 92, 96),
        Some(0x20b) => (u64_at(&optional, 24), 108, 112),
        _ => return Err(invalid("an unknown optional header")),
    };
    let image_base = image_base.ok_or_else(|| invalid("a short optional header"))?;
    let directory_count = u32_at(&optional, count_at).unwrap_or_default();
    let directory = |index: u32| {
        if index >= directory_count {
            return None;
        }
        let at = directories_at + 8 * index as usize;
        let rva = u32_at(&optional, at)?;
        let size = u32_at(&optional, at + 4)?;
        (rva != 0 && size != 0).then_some(u64::from(rva))
    };
    let table = read_at(
        program,
        nt + 24 + u64::from(optional_size),
        40 * usize::from(section_count),
    )?;
    let sections: Vec<Section> = table
        .as_chunks::<40>()
        .0
        .iter()
        .map(|header| Section {
            size: u64::from(u32_at(header, 8).unwrap_or_default()),
            address: u64::from(u32_at(header, 12).unwrap_or_default()),
            raw_size: u64::from(u32_at(header, 16).unwrap_or_default()),
            raw_offset: u64::from(u32_at(header, 20).unwrap_or_default()),
        })
        .collect();
    let mut dlls = Vec::new();
    // The import table: 20-byte descriptors, the DLL's name at 12, ended by
    // one of zeros.
    if let Some(rva) = directory(1) {
        for index in 0..MOST_DLLS {
            let offset = file_offset(&sections, rva + 20 * index)
                .ok_or_else(|| invalid("an import table outside its file"))?;
            let descriptor = read_at(program, offset, 20)?;
            if descriptor.iter().all(|&b| b == 0) {
                break;
            }
            let name = u64::from(u32_at(&descriptor, 12).unwrap_or_default());
            dlls.push(name_at(program, &sections, name)?);
        }
    }
    // The delay-load table: 32-byte descriptors, the DLL's name at 4, an
    // address in the image (bit 0 of the attributes) or, from old linkers,
    // in memory.
    if let Some(rva) = directory(13) {
        for index in 0..MOST_DLLS {
            let offset = file_offset(&sections, rva + 32 * index)
                .ok_or_else(|| invalid("a delay-load table outside its file"))?;
            let descriptor = read_at(program, offset, 32)?;
            if descriptor.iter().all(|&b| b == 0) {
                break;
            }
            let attributes = u32_at(&descriptor, 0).unwrap_or_default();
            let mut name = u64::from(u32_at(&descriptor, 4).unwrap_or_default());
            if attributes & 1 == 0 {
                name = name
                    .checked_sub(image_base)
                    .ok_or_else(|| invalid("a delay-loaded DLL name outside its image"))?;
            }
            dlls.push(name_at(program, &sections, name)?);
        }
    }
    Ok(dlls)
}

/// Whether a program importing `dlls` ([`imported_dlls`]) reads DirectInput.
pub fn direct_input(dlls: &[String]) -> bool {
    dlls.iter().any(|dll| DIRECT_INPUT.contains(&dll.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A PE32+ program with one section at 0x1000 (file offset 0x200)
    /// importing `imports` and delay-loading `delayed`.
    fn program(imports: &[&str], delayed: &[&str]) -> Vec<u8> {
        let mut file = vec![0u8; 0x400];
        let put32 = |file: &mut Vec<u8>, at: usize, value: u32| {
            file[at..at + 4].copy_from_slice(&value.to_le_bytes())
        };
        file[..2].copy_from_slice(b"MZ");
        put32(&mut file, 0x3c, 64);
        file[64..68].copy_from_slice(b"PE\0\0");
        file[64 + 6..64 + 8].copy_from_slice(&1u16.to_le_bytes());
        file[64 + 20..64 + 22].copy_from_slice(&240u16.to_le_bytes());
        let optional = 64 + 24;
        file[optional..optional + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        file[optional + 24..optional + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        put32(&mut file, optional + 108, 16);
        let section = optional + 240;
        put32(&mut file, section + 8, 0x200);
        put32(&mut file, section + 12, 0x1000);
        put32(&mut file, section + 16, 0x200);
        put32(&mut file, section + 20, 0x200);
        // In the section: imports at 0x1000, delay loads at 0x1080, names
        // from 0x1100.
        let at = |rva: u32| 0x200 + (rva - 0x1000) as usize;
        let mut name = 0x1100;
        let mut write_name = |file: &mut Vec<u8>, dll: &str| {
            let rva = name;
            file[at(rva)..at(rva) + dll.len()].copy_from_slice(dll.as_bytes());
            name += 0x20;
            rva
        };
        if !imports.is_empty() {
            put32(&mut file, optional + 112 + 8, 0x1000);
            put32(
                &mut file,
                optional + 112 + 12,
                20 * (imports.len() as u32 + 1),
            );
        }
        for (index, dll) in imports.iter().enumerate() {
            let rva = write_name(&mut file, dll);
            put32(&mut file, at(0x1000) + 20 * index + 12, rva);
        }
        if !delayed.is_empty() {
            put32(&mut file, optional + 112 + 8 * 13, 0x1080);
            put32(
                &mut file,
                optional + 112 + 8 * 13 + 4,
                32 * (delayed.len() as u32 + 1),
            );
        }
        for (index, dll) in delayed.iter().enumerate() {
            let rva = write_name(&mut file, dll);
            put32(&mut file, at(0x1080) + 32 * index, 1);
            put32(&mut file, at(0x1080) + 32 * index + 4, rva);
        }
        file
    }

    fn dlls(file: Vec<u8>) -> io::Result<Vec<String>> {
        imported_dlls(&mut Cursor::new(file))
    }

    /// NieR:Automata imports DINPUT8.dll; most games read no DirectInput.
    #[test]
    fn a_program_importing_direct_input_is_known_by_its_import_tables() {
        let nier = dlls(program(&["KERNEL32.dll", "DINPUT8.dll"], &[])).unwrap();
        assert_eq!(nier, ["kernel32.dll", "dinput8.dll"]);
        assert!(direct_input(&nier));
        let other = dlls(program(
            &["KERNEL32.dll", "XINPUT1_3.dll"],
            &["dbghelp.dll"],
        ))
        .unwrap();
        assert_eq!(other, ["kernel32.dll", "xinput1_3.dll", "dbghelp.dll"]);
        assert!(!direct_input(&other));
        let delayed = dlls(program(&["KERNEL32.dll"], &["DINPUT.dll"])).unwrap();
        assert!(direct_input(&delayed));
        assert_eq!(dlls(program(&[], &[])).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn a_file_that_is_no_program_or_is_cut_short_is_refused() {
        assert!(dlls(Vec::new()).is_err());
        assert!(dlls(b"#!/bin/sh\n".repeat(20)).is_err());
        let whole = program(&["DINPUT8.dll"], &[]);
        for len in [0x40, 0x100, 0x200, 0x300] {
            assert!(dlls(whole[..len].to_vec()).is_err(), "{len}");
        }
        // Headers pointing beyond the file.
        let mut far = whole.clone();
        far[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(dlls(far).is_err());
        let mut sections = whole.clone();
        sections[64 + 6..64 + 8].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(dlls(sections).is_err());
        let mut name = whole;
        name[0x200 + 12..0x200 + 16].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        assert!(dlls(name).is_err());
    }
}
