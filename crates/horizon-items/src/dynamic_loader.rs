use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicItemDef {
    pub id: String,
    #[serde(default = "default_max_stack")]
    pub max_stack: u32,
}

fn default_max_stack() -> u32 {
    64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub items: Vec<DynamicItemDef>,
}

#[derive(Debug, Default)]
pub struct DynamicModRegistry {
    pub loaded_namespaces: Vec<String>,
    pub loaded_mod_ids: Vec<String>,
    pub registered_items: Vec<(String, DynamicItemDef)>,
}

impl DynamicModRegistry {
    pub fn scan_mods_dir<P: AsRef<Path>>(mods_dir: P) -> Self {
        let mut registry = Self::default();
        let path = mods_dir.as_ref();

        if !path.exists() {
            let _ = fs::create_dir_all(path);
            return registry;
        }

        let Ok(entries) = fs::read_dir(path) else {
            return registry;
        };

        for entry in entries.flatten() {
            let mod_folder = entry.path();
            if !mod_folder.is_dir() {
                continue;
            }

            let manifest_path = mod_folder.join("mod.json");
            if !manifest_path.exists() {
                continue;
            }

            if let Ok(content) = fs::read_to_string(&manifest_path)
                && let Ok(manifest) = serde_json::from_str::<ModManifest>(&content)
            {
                println!(
                    "[DynamicModLoader] Loaded mod: {} v{}",
                    manifest.name, manifest.version
                );

                if !registry.loaded_namespaces.contains(&manifest.namespace) {
                    registry.loaded_namespaces.push(manifest.namespace.clone());
                }
                if !registry.loaded_mod_ids.contains(&manifest.id) {
                    registry.loaded_mod_ids.push(manifest.id.clone());
                }

                for item in manifest.items {
                    let full_id = format!("{}:{}", manifest.namespace, item.id);
                    registry.registered_items.push((full_id, item));
                }
            }
        }

        registry
    }
}
