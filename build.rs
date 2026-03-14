use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Evaluate simple integer expressions like "30*7*5" or "129".
fn eval_expr(s: &str) -> Option<u32> {
    if s.contains('*') {
        let mut result: u32 = 1;
        for part in s.split('*') {
            result = result.checked_mul(part.trim().parse::<u32>().ok()?)?;
        }
        Some(result)
    } else {
        s.parse::<u32>().ok()
    }
}

fn align_to(offset: usize, alignment: usize) -> usize {
    let rem = offset % alignment;
    if rem == 0 {
        offset
    } else {
        offset + alignment - rem
    }
}

/// Map C type name to (byte_size, is_signed).
fn parse_c_type(type_str: &str) -> Option<(usize, bool)> {
    match type_str {
        "uint32_t" => Some((4, false)),
        "int32_t" => Some((4, true)),
        "uint16_t" => Some((2, false)),
        "int16_t" => Some((2, true)),
        "uint8_t" => Some((1, false)),
        "int8_t" => Some((1, true)),
        _ => None,
    }
}

fn main() {
    println!("cargo:rerun-if-changed=gc2053/tiziano_params.h");

    let header = fs::read_to_string("gc2053/tiziano_params.h")
        .expect("Failed to read gc2053/tiziano_params.h");

    // Phase 1: Extract all #define NAME_COLS N (supports expressions like 30*7*5)
    let mut cols_map: HashMap<String, u32> = HashMap::new();
    for line in header.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("#define ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            if parts.len() == 2 && parts[0].ends_with("_COLS") {
                if let Some(n) = eval_expr(parts[1]) {
                    cols_map.insert(parts[0].to_string(), n);
                }
            }
        }
    }

    // Phase 2: Extract struct field declarations in order
    // Supports: uint32_t, int32_t, uint16_t, int16_t, uint8_t, int8_t
    // Accepts any TISP_ prefixed field name (TISP_PARAM_, TISP_ADR_, TISP_HISTSUB_, etc.)
    let mut fields: Vec<(String, u32, usize, bool)> = Vec::new(); // (name, cols, elem_bytes, signed)
    let mut in_struct = false;

    for line in header.lines() {
        let line = line.trim();
        if line.starts_with("} tisp_params_data_t;") {
            break;
        }
        if line == "typedef struct {" && !in_struct {
            in_struct = true;
            fields.clear();
            continue;
        }
        if !in_struct {
            continue;
        }

        // Try each supported C type
        let type_prefixes = [
            "uint32_t ",
            "int32_t ",
            "uint16_t ",
            "int16_t ",
            "uint8_t ",
            "int8_t ",
        ];
        for type_prefix in type_prefixes {
            if let Some(rest) = line.strip_prefix(type_prefix) {
                if let Some(bracket_pos) = rest.find('[') {
                    let name = &rest[..bracket_pos].trim();
                    if name.starts_with("TISP_") {
                        let type_str = type_prefix.trim();
                        let (elem_bytes, signed) =
                            parse_c_type(type_str).expect("Known type failed to parse");
                        let cols_key = format!("{}_COLS", name);
                        let cols = cols_map.get(&cols_key).copied().unwrap_or_else(|| {
                            panic!("No COLS define found for field {}", name);
                        });
                        fields.push((name.to_string(), cols, elem_bytes, signed));
                    }
                }
                break; // Only one type prefix can match per line
            }
        }
    }

    assert!(
        !fields.is_empty(),
        "No fields extracted from tiziano_params.h"
    );

    // Phase 3: Compute struct layout with C alignment rules
    let mut offset: usize = 0;
    for &(_, cols, elem_bytes, _) in &fields {
        offset = align_to(offset, elem_bytes);
        offset += cols as usize * elem_bytes;
    }
    // Final struct alignment (max member alignment = 4)
    let total_bytes = align_to(offset, 4);

    // Phase 4: Generate schema.rs
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("schema.rs");
    let mut f = fs::File::create(&out_path).unwrap();

    writeln!(f, "/// Auto-generated from tiziano_params.h").unwrap();
    writeln!(
        f,
        "/// Each entry: (field_name, num_elements, element_byte_size, is_signed)"
    )
    .unwrap();
    writeln!(f, "pub const SCHEMA: &[(&str, usize, usize, bool)] = &[").unwrap();
    for (name, cols, elem_bytes, signed) in &fields {
        writeln!(
            f,
            "    (\"{}\", {}, {}, {}),",
            name, cols, elem_bytes, signed
        )
        .unwrap();
    }
    writeln!(f, "];").unwrap();

    writeln!(f).unwrap();
    writeln!(
        f,
        "/// Total bytes in one params_data block (including alignment padding)"
    )
    .unwrap();
    writeln!(f, "pub const PARAMS_DATA_BYTES: usize = {};", total_bytes).unwrap();

    eprintln!(
        "tiziano-params: extracted {} fields, {} bytes per profile",
        fields.len(),
        total_bytes
    );
}
