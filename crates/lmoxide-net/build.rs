use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../..");
    let proto_dir = root.join("proto");
    let protos: Vec<PathBuf> = fs::read_dir(&proto_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "proto"))
        .collect();
    for p in &protos {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    prost_build::Config::new()
        .compile_protos(&protos, &[&proto_dir])
        .expect("compile protos");

    // RPC method tables extracted from LMO.exe by tools/re/extract_rpc.py
    let methods_json = root.join("data/rpc_methods.json");
    println!("cargo:rerun-if-changed={}", methods_json.display());
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&methods_json).unwrap()).unwrap();
    let mut out = String::from("// @generated from data/rpc_methods.json\n\n");
    let mut all = Vec::new();
    for (section, suffix) in [("receivers", "Receiver"), ("senders", "Sender")] {
        for (class, table) in v[section].as_object().unwrap() {
            let iface = class.strip_suffix(suffix).unwrap();
            let const_name = to_screaming_snake(iface);
            writeln!(out, "/// `{iface}` methods as `(method id, C++ signature)`.").unwrap();
            writeln!(out, "pub const {const_name}: &[(u32, &str)] = &[").unwrap();
            let mut rows: Vec<(u32, &str)> = table
                .as_object()
                .unwrap()
                .iter()
                .map(|(k, sig)| {
                    let id = match k.strip_prefix("0x") {
                        Some(h) => u32::from_str_radix(h, 16).unwrap(),
                        None => k.parse().unwrap(),
                    };
                    (id, sig.as_str().unwrap())
                })
                .collect();
            rows.sort();
            for (id, sig) in &rows {
                writeln!(out, "    (0x{id:08x}, {sig:?}),").unwrap();
            }
            writeln!(out, "];\n").unwrap();
            // Named constants: `methods::player_agent::AUTHENTICATE`.
            writeln!(out, "pub mod {} {{", const_name.to_lowercase()).unwrap();
            let mut seen = std::collections::HashMap::<String, u32>::new();
            for (id, sig) in &rows {
                let name = sig.split('(').next().unwrap().trim();
                let mut c = to_screaming_snake(name);
                let n = seen.entry(c.clone()).or_default();
                *n += 1;
                if *n > 1 {
                    c = format!("{c}_{n}");
                }
                writeln!(out, "    /// `{sig}`").unwrap();
                writeln!(out, "    pub const {c}: u32 = 0x{id:08x};").unwrap();
            }
            writeln!(out, "}}\n").unwrap();
            all.push((iface.to_string(), const_name));
        }
    }
    writeln!(out, "/// All interfaces, by endpoint name.").unwrap();
    writeln!(out, "pub const INTERFACES: &[(&str, &[(u32, &str)])] = &[").unwrap();
    for (iface, c) in all {
        writeln!(out, "    ({iface:?}, {c}),").unwrap();
    }
    writeln!(out, "];").unwrap();
    fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("methods.rs"), out).unwrap();

    // IIR names/type ids extracted from LMO.exe by tools/re/extract_iir.py
    let iir_json = root.join("data/iir_types.json");
    println!("cargo:rerun-if-changed={}", iir_json.display());
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&iir_json).unwrap()).unwrap();
    let mut out = String::from("// @generated from data/iir_types.json\n/// `(type id, registered name)` of every IIR the client knows.\npub const IIR_TYPES: &[(u32, &str)] = &[\n");
    let mut rows: Vec<(u32, String)> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            let id = u32::from_str_radix(e["type_id"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap();
            (id, e["name"].as_str().unwrap().to_string())
        })
        .collect();
    rows.sort();
    for (id, name) in rows {
        writeln!(out, "    (0x{id:08x}, {name:?}),").unwrap();
    }
    out.push_str("];\n");
    fs::write(PathBuf::from(env::var("OUT_DIR").unwrap()).join("iir_types.rs"), out).unwrap();
}

fn to_screaming_snake(s: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        if i > 0 && c.is_uppercase() && (prev_lower || next_lower) {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}
