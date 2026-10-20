use crate::state::Shared;
use anyhow::{Result, ensure};
use butterpollo_core::library::{self, Selection};
use serde_json::{Value, json};

pub fn discovery(h: &Shared) -> Result<Value> {
    let mut catalog = butterpollo_windows::library::discover();
    let apps: Vec<Value> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    let mut games = vec![];
    for game in &mut catalog.games {
        let existing = apps.iter().find(|app| game.matches(app));
        game.already_added = existing.is_some();
        let mut value = serde_json::to_value(&*game)?;
        value["uuid"] = existing
            .and_then(|app| app.get("uuid"))
            .cloned()
            .unwrap_or_else(|| json!(game.uuid()));
        value["has_cover"] = json!(
            existing.is_some_and(|app| app["image-path"].as_str().is_some_and(|s| !s.is_empty()))
        );
        games.push(value);
    }
    Ok(json!({ "status": true, "sources": catalog.sources, "games": games }))
}

pub fn import(h: &Shared, selected: &[Selection]) -> Result<Value> {
    ensure!(!selected.is_empty(), "Select at least one installed game.");
    import_games(h, butterpollo_windows::library::discover().games, selected)
}

fn import_games(
    h: &Shared,
    mut games: Vec<library::Game>,
    selected: &[Selection],
) -> Result<Value> {
    let existing: Vec<Value> = h
        .apps
        .read()
        .unwrap()
        .iter()
        .map(serde_json::to_value)
        .collect::<Result<_, _>>()?;
    for game in &mut games {
        if existing.iter().any(|app| game.matches(app)) {
            continue;
        }
        if !selected
            .iter()
            .any(|entry| entry.source == game.source && entry.id == game.id)
        {
            continue;
        }
        if let Some(source) = game.cover.take() {
            let destination = h
                .directory
                .join("covers")
                .join(format!("launcher_{}.png", game.uuid()));
            match butterpollo_windows::image::to_png(&source, &destination) {
                Ok(_) => game.cover = Some(destination),
                Err(error) => {
                    tracing::debug!(%error, source = %source.display(), "Launcher cover unavailable")
                }
            }
        }
    }
    let mut results = vec![];
    crate::steam::update_apps(h, |apps| {
        results = library::import(apps, &games, selected);
        results.iter().any(|result| result.result == "added")
    })?;
    Ok(json!({ "status": true, "results": results }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;
    use std::path::Path;

    #[test]
    fn imports_persist_without_replacing_document_metadata_or_existing_customizations() {
        let f = Fixture::new();
        f.host.app_document.write().unwrap()["custom-root"] = json!({ "keep": true });
        let game = library::ubisoft("13504", Path::new("F:/Games/Mirage"), "Mirage").unwrap();
        let selection = Selection {
            source: game.source,
            id: game.id.clone(),
        };
        assert_eq!(
            import_games(
                &f.host,
                vec![game.clone()],
                std::slice::from_ref(&selection)
            )
            .unwrap()["results"][0]["result"],
            "added"
        );
        crate::steam::update_apps(&f.host, |apps| {
            let app = apps.iter_mut().find(|app| game.matches(app)).unwrap();
            app["name"] = json!("My Mirage");
            app["custom"] = json!([1, 2, 3]);
            app["cmd"] = json!("my command");
            true
        })
        .unwrap();
        let before = std::fs::read(&f.host.apps_path).unwrap();
        assert_eq!(
            import_games(&f.host, vec![game], std::slice::from_ref(&selection)).unwrap()["results"]
                [0]["result"],
            "already_present"
        );
        assert_eq!(
            import_games(&f.host, vec![], &[selection]).unwrap()["results"][0]["result"],
            "failed"
        );
        assert_eq!(std::fs::read(&f.host.apps_path).unwrap(), before);
        let saved: Value = serde_json::from_slice(&before).unwrap();
        assert_eq!(saved["custom-root"], json!({ "keep": true }));
        assert!(
            saved["apps"]
                .as_array()
                .unwrap()
                .iter()
                .any(|app| app["name"] == "My Mirage" && app["custom"] == json!([1, 2, 3]))
        );
    }
}
