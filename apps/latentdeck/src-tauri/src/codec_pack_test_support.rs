use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use latentdeck_cartridge::hash::hash_reader;
use serde_json::json;

fn sha256(bytes: &[u8]) -> String {
    hash_reader(&mut Cursor::new(bytes))
        .expect("synthetic fixture hash")
        .sha256
        .to_string()
}

pub(crate) fn write_h3_pack(root: &Path, version: &str) -> PathBuf {
    let pack = root.join("org.latentdeck.h3").join(version);
    fs::create_dir_all(pack.join("bin")).expect("codec pack fixture directories");
    let worker = b"synthetic worker";
    let notice = b"synthetic notice";
    fs::write(pack.join("bin/worker.exe"), worker).expect("synthetic worker");
    fs::write(pack.join("NOTICE.txt"), notice).expect("synthetic notice");
    let catalog = json!({
        "manifest_version": "1.0.0",
        "files": [
            {"path": "NOTICE.txt", "byte_length": notice.len(), "sha256": sha256(notice)},
            {"path": "bin/worker.exe", "byte_length": worker.len(), "sha256": sha256(worker)}
        ]
    });
    let catalog_bytes = serde_json::to_vec(&catalog).expect("catalog JSON");
    fs::write(pack.join("catalog.json"), &catalog_bytes).expect("catalog");
    let manifest = json!({
        "manifest_version": "1.0.0",
        "pack_id": "org.latentdeck.h3",
        "pack_version": version,
        "display_name": "Synthetic H3 Codec Pack",
        "publisher": {"name": "LatentDeck Test", "url": null},
        "license": {"spdx_or_label": "MIT", "notice_path": "NOTICE.txt"},
        "platform": {"os": "windows", "arch": "x86_64"},
        "compatibility": {
            "app_min_inclusive": "0.1.0",
            "app_max_exclusive": "0.2.0",
            "worker_protocol_min": 1,
            "worker_protocol_max": 1,
            "lc_spec_versions": ["0.1.0"],
            "profiles": [{
                "codec_family": "minimax_h3",
                "profile": "h3_av_latent",
                "profile_versions": ["0.1.0"]
            }]
        },
        "worker": {
            "executable": "bin/worker.exe",
            "arguments": ["--worker"],
            "d2_arguments": ["--d2-worker"],
            "q4_arguments": ["--q4-worker"],
            "working_directory": "bin",
            "probe_timeout_ms": 5000
        },
        "adapter": {"adapter_id": "org.latentdeck.h3", "adapter_version": "0.1.0"},
        "integrity": {"catalog_path": "catalog.json", "catalog_sha256": sha256(&catalog_bytes)},
        "external_assets": []
    });
    fs::write(
        pack.join("codec-pack.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest JSON"),
    )
    .expect("manifest");
    pack
}
