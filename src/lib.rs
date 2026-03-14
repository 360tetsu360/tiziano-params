include!(concat!(env!("OUT_DIR"), "/schema.rs"));

use std::collections::BTreeMap;

/// Version string at the start of the bin file.
const VERSION: &[u8; 8] = b"2.10\0w+\0";
/// Header flag.
const HEADER_FLAG: &[u8; 8] = b"header0\0";
/// Header size: flag(8) + size(4) + crc(4) = 16 bytes.
const HEADER_SIZE: usize = 16;
/// Version field size.
const VERSION_SIZE: usize = 8;
/// Data offset: version + header.
const DATA_OFFSET: usize = VERSION_SIZE + HEADER_SIZE;

pub type ParamMap = BTreeMap<String, Vec<i64>>;

#[derive(Debug, Clone)]
pub struct IspParams {
    pub day: ParamMap,
    pub night: ParamMap,
    /// Bytes beyond known schema (preserved for roundtrip fidelity).
    pub day_tail: Vec<u8>,
    pub night_tail: Vec<u8>,
    /// Actual bytes per profile in the bin.
    pub profile_bytes: usize,
}

/// CRC32 matching the C implementation (8-entry table, operating on u32 words).
fn crc32_words(data: &[u8]) -> u32 {
    const TABLE: [u32; 8] = [
        0x00000000, 0x77073096, 0xee0e612c, 0x990951ba, 0x076dc419, 0x706af48f, 0xe963a535,
        0x9e6495a3,
    ];
    let mut crc: u32 = TABLE[0];
    // Process as little-endian u32 words
    for chunk in data.chunks_exact(4) {
        let word = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        crc ^= word;
        crc ^= TABLE[(crc & 0x7) as usize];
    }
    crc
}

fn read_u32_le(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn align_to(offset: usize, alignment: usize) -> usize {
    let rem = offset % alignment;
    if rem == 0 {
        offset
    } else {
        offset + alignment - rem
    }
}

/// Read one element from raw bytes based on type info.
fn read_element(data: &[u8], offset: usize, elem_bytes: usize, signed: bool) -> i64 {
    match (elem_bytes, signed) {
        (4, false) => u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as i64,
        (4, true) => i32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]) as i64,
        (2, false) => u16::from_le_bytes([data[offset], data[offset + 1]]) as i64,
        (2, true) => i16::from_le_bytes([data[offset], data[offset + 1]]) as i64,
        (1, false) => data[offset] as i64,
        (1, true) => data[offset] as i8 as i64,
        _ => panic!("Unsupported type: {} bytes, signed={}", elem_bytes, signed),
    }
}

/// Write one element to a byte buffer based on type info.
fn write_element(buf: &mut Vec<u8>, val: i64, elem_bytes: usize, signed: bool) {
    match (elem_bytes, signed) {
        (4, false) => buf.extend_from_slice(&(val as u32).to_le_bytes()),
        (4, true) => buf.extend_from_slice(&(val as i32).to_le_bytes()),
        (2, false) => buf.extend_from_slice(&(val as u16).to_le_bytes()),
        (2, true) => buf.extend_from_slice(&(val as i16).to_le_bytes()),
        (1, false) => buf.push(val as u8),
        (1, true) => buf.push(val as i8 as u8),
        _ => panic!("Unsupported type: {} bytes, signed={}", elem_bytes, signed),
    }
}

/// Decode a params_data block (one profile) from raw bytes.
/// Returns (named fields, trailing unknown bytes).
fn decode_profile(data: &[u8]) -> (ParamMap, Vec<u8>) {
    let mut map = ParamMap::new();
    let mut offset = 0;
    for &(name, cols, elem_bytes, signed) in SCHEMA {
        // Align offset to element's natural alignment
        offset = align_to(offset, elem_bytes);
        let mut vals = Vec::with_capacity(cols);
        for i in 0..cols {
            vals.push(read_element(
                data,
                offset + i * elem_bytes,
                elem_bytes,
                signed,
            ));
        }
        map.insert(name.to_string(), vals);
        offset += cols * elem_bytes;
    }
    // Align to struct alignment (4) for the tail boundary
    offset = align_to(offset, 4);
    // Any remaining bytes are unknown fields (firmware newer than header)
    let tail = data[offset..].to_vec();
    (map, tail)
}

/// Encode a params_data block (one profile) to raw bytes.
fn encode_profile(map: &ParamMap, tail: &[u8], profile_bytes: usize) -> Vec<u8> {
    let mut buf = Vec::with_capacity(profile_bytes);
    for &(name, cols, elem_bytes, signed) in SCHEMA {
        // Insert alignment padding (zeros)
        let aligned = align_to(buf.len(), elem_bytes);
        buf.resize(aligned, 0);

        let vals = map
            .get(name)
            .unwrap_or_else(|| panic!("Missing field: {}", name));
        assert_eq!(
            vals.len(),
            cols,
            "Field {} expected {} elements, got {}",
            name,
            cols,
            vals.len()
        );
        for &v in vals {
            write_element(&mut buf, v, elem_bytes, signed);
        }
    }
    // Align to struct alignment before appending tail
    let aligned = align_to(buf.len(), 4);
    buf.resize(aligned, 0);
    buf.extend_from_slice(tail);
    buf
}

/// Strip TISP prefix for TOML display.
fn short_name(name: &str) -> &str {
    name.strip_prefix("TISP_PARAM_")
        .or_else(|| name.strip_prefix("TISP_"))
        .unwrap_or(name)
}

/// Decode a gc2053-t31.bin file into structured params.
pub fn decode(bin: &[u8]) -> Result<IspParams, String> {
    if bin.len() < DATA_OFFSET {
        return Err("File too small for header".into());
    }

    // Validate version
    if &bin[..VERSION_SIZE] != VERSION {
        return Err(format!(
            "Version mismatch: expected {:?}, got {:?}",
            VERSION,
            &bin[..VERSION_SIZE]
        ));
    }

    // Read header
    let flag = &bin[VERSION_SIZE..VERSION_SIZE + 8];
    if flag != HEADER_FLAG {
        return Err(format!("Header flag mismatch: {:?}", flag));
    }

    let size = read_u32_le(bin, VERSION_SIZE + 8) as usize;
    let stored_crc = read_u32_le(bin, VERSION_SIZE + 12);

    if size % 2 != 0 {
        return Err(format!("Size {} is not even (need day+night)", size));
    }
    let profile_bytes = size / 2;

    if profile_bytes < PARAMS_DATA_BYTES {
        return Err(format!(
            "Profile too small: {} bytes, schema needs at least {}",
            profile_bytes, PARAMS_DATA_BYTES
        ));
    }

    if bin.len() < DATA_OFFSET + size {
        return Err(format!(
            "File truncated: need {} bytes, have {}",
            DATA_OFFSET + size,
            bin.len()
        ));
    }

    let data = &bin[DATA_OFFSET..DATA_OFFSET + size];

    // Verify CRC
    let computed_crc = crc32_words(data);
    if computed_crc != stored_crc {
        return Err(format!(
            "CRC mismatch: stored 0x{:08x}, computed 0x{:08x}",
            stored_crc, computed_crc
        ));
    }

    let (day, day_tail) = decode_profile(&data[..profile_bytes]);
    let (night, night_tail) = decode_profile(&data[profile_bytes..]);

    if !day_tail.is_empty() {
        eprintln!(
            "Note: {} unknown bytes per profile (firmware newer than header)",
            day_tail.len()
        );
    }

    Ok(IspParams {
        day,
        night,
        day_tail,
        night_tail,
        profile_bytes,
    })
}

/// Encode structured params into a gc2053-t31.bin file.
pub fn encode(params: &IspParams) -> Vec<u8> {
    let day_data = encode_profile(&params.day, &params.day_tail, params.profile_bytes);
    let night_data = encode_profile(&params.night, &params.night_tail, params.profile_bytes);

    let size = (day_data.len() + night_data.len()) as u32;
    let mut payload = Vec::with_capacity(size as usize);
    payload.extend_from_slice(&day_data);
    payload.extend_from_slice(&night_data);

    let crc = crc32_words(&payload);

    let mut bin = Vec::with_capacity(DATA_OFFSET + payload.len());
    // Version
    bin.extend_from_slice(VERSION);
    // Header: flag + size + crc
    bin.extend_from_slice(HEADER_FLAG);
    bin.extend_from_slice(&size.to_le_bytes());
    bin.extend_from_slice(&crc.to_le_bytes());
    // Data
    bin.extend_from_slice(&payload);

    bin
}

/// Convert params to TOML string.
pub fn to_toml(params: &IspParams) -> String {
    let mut out = String::new();
    out.push_str("# ISP Parameters — auto-generated from gc2053-t31.bin\n");
    out.push_str(
        "# Edit values and re-encode with: tiziano-params encode params.toml output.bin\n\n",
    );

    // Metadata section for roundtrip
    out.push_str("[meta]\n");
    out.push_str(&format!("profile_bytes = {}\n", params.profile_bytes));
    if !params.day_tail.is_empty() {
        out.push_str(&format!("day_tail = {:?}\n", hex_encode(&params.day_tail)));
    }
    if !params.night_tail.is_empty() {
        out.push_str(&format!(
            "night_tail = {:?}\n",
            hex_encode(&params.night_tail)
        ));
    }
    out.push('\n');

    for (section, map) in [("day", &params.day), ("night", &params.night)] {
        out.push_str(&format!("[{}]\n", section));
        for &(name, _, _, _) in SCHEMA {
            if let Some(vals) = map.get(name) {
                let short = short_name(name);
                let vals_str: Vec<String> = vals.iter().map(|v| v.to_string()).collect();
                out.push_str(&format!("{} = [{}]\n", short, vals_str.join(", ")));
            }
        }
        out.push('\n');
    }

    out
}

fn hex_encode(data: &[u8]) -> String {
    data.iter().map(|b| format!("{:02x}", b)).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, String> {
    if s.len() % 2 != 0 {
        return Err("Hex string has odd length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| format!("Bad hex at offset {}: {}", i, e))
        })
        .collect()
}

/// Parse TOML string back into params.
pub fn from_toml(toml_str: &str) -> Result<IspParams, String> {
    let mut day = ParamMap::new();
    let mut night = ParamMap::new();
    let mut day_tail = Vec::new();
    let mut night_tail = Vec::new();
    let mut profile_bytes = PARAMS_DATA_BYTES;

    // Build short-name → schema entry lookup
    let short_to_schema: BTreeMap<String, (&str, usize, usize, bool)> = SCHEMA
        .iter()
        .map(|&entry| {
            let sn = short_name(entry.0).to_string();
            (sn, entry)
        })
        .collect();

    enum Section {
        Meta,
        Day,
        Night,
    }
    let mut current_section: Option<Section> = None;

    for (line_no, line) in toml_str.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line == "[meta]" {
            current_section = Some(Section::Meta);
            continue;
        }
        if line == "[day]" {
            current_section = Some(Section::Day);
            continue;
        }
        if line == "[night]" {
            current_section = Some(Section::Night);
            continue;
        }

        let (key, val_str) = line
            .split_once('=')
            .ok_or_else(|| format!("Line {}: expected KEY = VALUE", line_no + 1))?;
        let key = key.trim();
        let val_str = val_str.trim();

        match &current_section {
            None => return Err(format!("Line {}: data before section header", line_no + 1)),
            Some(Section::Meta) => {
                match key {
                    "profile_bytes" => {
                        profile_bytes = val_str.parse().map_err(|e| {
                            format!("Line {}: bad profile_bytes: {}", line_no + 1, e)
                        })?;
                    }
                    "day_tail" => {
                        let hex = val_str.trim_matches('"');
                        day_tail =
                            hex_decode(hex).map_err(|e| format!("Line {}: {}", line_no + 1, e))?;
                    }
                    "night_tail" => {
                        let hex = val_str.trim_matches('"');
                        night_tail =
                            hex_decode(hex).map_err(|e| format!("Line {}: {}", line_no + 1, e))?;
                    }
                    _ => {} // ignore unknown meta keys
                }
            }
            Some(Section::Day) | Some(Section::Night) => {
                let map = match &current_section {
                    Some(Section::Day) => &mut day,
                    Some(Section::Night) => &mut night,
                    _ => unreachable!(),
                };

                let &(full_name, expected_cols, elem_bytes, signed) = short_to_schema
                    .get(key)
                    .ok_or_else(|| format!("Line {}: unknown field '{}'", line_no + 1, key))?;

                let inner = val_str
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .unwrap_or(val_str);

                let vals: Vec<i64> = inner
                    .split(',')
                    .map(|s| {
                        s.trim().parse::<i64>().map_err(|e| {
                            format!("Line {}: parse error '{}': {}", line_no + 1, s.trim(), e)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;

                if vals.len() != expected_cols {
                    return Err(format!(
                        "Line {}: {} expected {} values, got {}",
                        line_no + 1,
                        key,
                        expected_cols,
                        vals.len()
                    ));
                }

                // Validate value ranges
                for &v in &vals {
                    let (min, max): (i64, i64) = match (elem_bytes, signed) {
                        (4, false) => (0, u32::MAX as i64),
                        (4, true) => (i32::MIN as i64, i32::MAX as i64),
                        (2, false) => (0, u16::MAX as i64),
                        (2, true) => (i16::MIN as i64, i16::MAX as i64),
                        (1, false) => (0, u8::MAX as i64),
                        (1, true) => (i8::MIN as i64, i8::MAX as i64),
                        _ => unreachable!(),
                    };
                    if v < min || v > max {
                        return Err(format!(
                            "Line {}: value {} out of range [{}, {}] for field {}",
                            line_no + 1,
                            v,
                            min,
                            max,
                            key
                        ));
                    }
                }

                map.insert(full_name.to_string(), vals);
            }
        }
    }

    for &(name, _, _, _) in SCHEMA {
        if !day.contains_key(name) {
            return Err(format!("Missing field in [day]: {}", name));
        }
        if !night.contains_key(name) {
            return Err(format!("Missing field in [night]: {}", name));
        }
    }

    Ok(IspParams {
        day,
        night,
        day_tail,
        night_tail,
        profile_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_sanity() {
        assert!(SCHEMA.len() > 100, "Schema should have many fields");
        assert!(
            PARAMS_DATA_BYTES > 10000,
            "Params data should be substantial"
        );
        // All names should start with TISP_
        for &(name, cols, elem_bytes, _) in SCHEMA {
            assert!(name.starts_with("TISP_"), "Bad name: {}", name);
            assert!(cols > 0, "Zero cols for {}", name);
            assert!(
                [1, 2, 4].contains(&elem_bytes),
                "Bad elem_bytes {} for {}",
                elem_bytes,
                name
            );
        }
    }

    #[test]
    fn roundtrip_bin() {
        let bin = std::fs::read("gc2053/gc2053-t31.bin").expect("Need gc2053-t31.bin for test");
        let params = decode(&bin).expect("decode failed");
        let reencoded = encode(&params);
        assert_eq!(bin.len(), reencoded.len(), "Size mismatch");
        assert_eq!(bin, reencoded, "Roundtrip mismatch");
    }

    #[test]
    fn roundtrip_toml() {
        let bin = std::fs::read("gc2053/gc2053-t31.bin").expect("Need gc2053-t31.bin for test");
        let params = decode(&bin).expect("decode failed");
        let toml = to_toml(&params);
        let params2 = from_toml(&toml).expect("from_toml failed");
        let reencoded = encode(&params2);
        assert_eq!(bin, reencoded, "TOML roundtrip mismatch");
    }

    #[test]
    fn crc_matches() {
        let bin = std::fs::read("gc2053/gc2053-t31.bin").expect("Need gc2053-t31.bin for test");
        let stored_crc = read_u32_le(&bin, VERSION_SIZE + 12);
        let size = read_u32_le(&bin, VERSION_SIZE + 8) as usize;
        let data = &bin[DATA_OFFSET..DATA_OFFSET + size];
        let computed = crc32_words(data);
        assert_eq!(stored_crc, computed, "CRC mismatch");
    }
}
