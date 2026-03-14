use std::fs;
use std::process;

fn usage() -> ! {
    eprintln!("Usage:");
    eprintln!("  tiziano-params decode <input.bin> <output.toml>");
    eprintln!("  tiziano-params encode <input.toml> <output.bin>");
    eprintln!("  tiziano-params info   <input.bin>");
    process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
    }

    match args[1].as_str() {
        "decode" => {
            if args.len() != 4 {
                usage();
            }
            let bin = fs::read(&args[2]).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", args[2], e);
                process::exit(1);
            });
            let params = tiziano_params::decode(&bin).unwrap_or_else(|e| {
                eprintln!("Decode error: {}", e);
                process::exit(1);
            });
            let toml = tiziano_params::to_toml(&params);
            fs::write(&args[3], &toml).unwrap_or_else(|e| {
                eprintln!("Failed to write {}: {}", args[3], e);
                process::exit(1);
            });
            eprintln!(
                "Decoded {} → {} ({} fields per profile)",
                args[2],
                args[3],
                tiziano_params::SCHEMA.len()
            );
        }
        "encode" => {
            if args.len() != 4 {
                usage();
            }
            let toml_str = fs::read_to_string(&args[2]).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", args[2], e);
                process::exit(1);
            });
            let params = tiziano_params::from_toml(&toml_str).unwrap_or_else(|e| {
                eprintln!("Parse error: {}", e);
                process::exit(1);
            });
            let bin = tiziano_params::encode(&params);
            fs::write(&args[3], &bin).unwrap_or_else(|e| {
                eprintln!("Failed to write {}: {}", args[3], e);
                process::exit(1);
            });
            eprintln!("Encoded {} → {} ({} bytes)", args[2], args[3], bin.len());
        }
        "info" => {
            if args.len() != 3 {
                usage();
            }
            let bin = fs::read(&args[2]).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {}", args[2], e);
                process::exit(1);
            });
            let params = tiziano_params::decode(&bin).unwrap_or_else(|e| {
                eprintln!("Decode error: {}", e);
                process::exit(1);
            });
            println!("File: {} ({} bytes)", args[2], bin.len());
            println!(
                "Schema: {} fields, {} known bytes per profile",
                tiziano_params::SCHEMA.len(),
                tiziano_params::PARAMS_DATA_BYTES
            );
            println!(
                "Profile: {} bytes ({} unknown tail bytes)",
                params.profile_bytes,
                params.day_tail.len()
            );

            // Show a few key params
            let interesting = [
                "TISP_PARAM_TOP_BYPASS",
                "TISP_PARAM_AE_EV_LIST",
                "TISP_PARAM_AE_AT_LIST",
                "TISP_PARAM_AE_EXP_PARAMETER",
            ];
            for name in interesting {
                let short = name.strip_prefix("TISP_PARAM_").unwrap_or(name);
                if let Some(d) = params.day.get(name) {
                    let vals: Vec<String> = d.iter().map(|v| v.to_string()).collect();
                    println!("  day.{} = [{}]", short, vals.join(", "));
                }
                if let Some(n) = params.night.get(name) {
                    let vals: Vec<String> = n.iter().map(|v| v.to_string()).collect();
                    println!("  night.{} = [{}]", short, vals.join(", "));
                }
            }
        }
        _ => usage(),
    }
}
