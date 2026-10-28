//! The world editor's published set: placements, trigger walls, notes, scripts and smash rules.

use std::path::Path;

use anyhow::Result;
use lmoxide_survey as sv;
use serde_json::json;

use super::*;
use crate::action::{Action, Destination};

pub fn survey(
    tree: &mut Tree,
    ctx: &mut Ctx,
    dir: &Path,
    data: &dyn Fn(&str) -> Result<Value>,
) -> Result<()> {
    let npc_spells = data("npc_spells.json")?;
    let npc_life = data("npc_life.json")?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .collect();
    files.sort();
    let mut t = std::mem::take(&mut ctx.templates);

    // scripts first: placements and walls name them
    let mut notes: Vec<(u32, sv::NotesFile)> = Vec::new();
    for f in files
        .iter()
        .filter(|f| f.to_string_lossy().ends_with(".notes.json"))
    {
        let n: sv::NotesFile = serde_json::from_value(read_json(f)?)?;
        for s in &n.scripts {
            let id = ctx.ids.new_id(
                &format!("script/{}", n.playfield),
                if s.name.is_empty() { &s.id } else { &s.name },
            );
            ctx.script.insert(s.id.clone(), id);
        }
        notes.push((n.playfield, n));
    }
    let mut note_area = std::collections::HashMap::new();
    for (pf, n) in &notes {
        let Some(world) = ctx.world.get(pf).cloned() else {
            tree.notes
                .push(format!("survey notes of unknown playfield {pf}"));
            continue;
        };
        for note in &n.notes {
            let id = ctx
                .ids
                .new_id(&format!("area/{pf}"), &format!("note {}", note.id));
            note_area.insert(note.id.clone(), id.clone());
            let area = Area {
                id,
                name: note.reference.clone().unwrap_or_default(),
                role: AreaRole::Note,
                shape: Some(shape(&note.area)),
                note: Some(
                    json!({ "kind": note.kind, "reference": note.reference, "status": note.status }),
                ),
                comment: note.comment.clone(),
                tags: note.tags.clone(),
                feature: note.feature.clone(),
                author: note.author.clone(),
                updated: note.updated,
                source: Some(Source::Surveyed),
                ..Default::default()
            };
            tree.areas.entry(world.clone()).or_default().push(area);
        }
        for s in &n.scripts {
            let id = ctx.script[&s.id].clone();
            let area = s.area.as_ref().map(|a| {
                let aid = ctx
                    .ids
                    .new_id(&format!("area/{pf}"), &format!("script {}", s.name));
                tree.areas.entry(world.clone()).or_default().push(Area {
                    id: aid.clone(),
                    name: s.name.clone(),
                    role: AreaRole::Note,
                    shape: Some(shape(a)),
                    feature: s.feature.clone(),
                    source: Some(Source::Surveyed),
                    ..Default::default()
                });
                aid
            });
            let waves = s
                .waves
                .iter()
                .map(|w| Wave {
                    id: w.id.clone(),
                    name: w.name.clone(),
                    trigger: w.trigger.as_ref().map(|tr| match tr {
                        sv::Trigger::After { seconds } => {
                            WaveTrigger::After(crate::Duration(f64::from(*seconds)))
                        }
                        sv::Trigger::HealthBelow { placement, percent } => {
                            WaveTrigger::HealthBelow {
                                spawn: placement.as_ref().map(|p| format!("spawn/{pf}/{p}")),
                                share: crate::Ratio(f64::from(*percent) / 100.0),
                            }
                        }
                        sv::Trigger::EnterArea { note } => WaveTrigger::EnterArea(
                            note.as_ref().and_then(|n| note_area.get(n).cloned()),
                        ),
                        sv::Trigger::WaveCleared { wave } => WaveTrigger::WaveCleared(wave.clone()),
                        sv::Trigger::Other => WaveTrigger::Other,
                    }),
                    comment: w.comment.clone(),
                    tags: w.tags.clone(),
                    ..Default::default()
                })
                .collect();
            tree.scripts.entry(world.clone()).or_default().push(Script {
                id,
                name: s.name.clone(),
                area,
                reference: s.reference.clone(),
                status: Some(
                    serde_json::to_value(s.status)?
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                ),
                waves,
                comment: s.comment.clone(),
                tags: s.tags.clone(),
                feature: s.feature.clone(),
                author: s.author.clone(),
                updated: s.updated,
                source: Some(Source::Surveyed),
                ..Default::default()
            });
        }
    }

    for f in files
        .iter()
        .filter(|f| f.to_string_lossy().ends_with(".placements.json"))
    {
        let p: sv::PlacementsFile = serde_json::from_value(read_json(f)?)?;
        let pf = p.playfield;
        let Some(world) = ctx.world.get(&pf).cloned() else {
            tree.notes
                .push(format!("survey placements of unknown playfield {pf}"));
            continue;
        };
        for pl in &p.placements {
            let replaces = pl
                .replaces
                .as_ref()
                .and_then(|o| find_origin(tree, &world, o));
            if pl.replaces.is_some() && replaces.is_none() {
                tree.notes.push(format!(
                    "survey placement {} in {pf}: the spawn it replaces wasn't found",
                    pl.id
                ));
            }
            if pl.kind == sv::Kind::Bouncer {
                let template = t.object(
                    tree,
                    ctx,
                    format!("bouncer|survey|{:?}", pl.template),
                    &format!("bouncer {}", pl.template.unwrap_or(0)),
                    |id| Object {
                        id,
                        kind: ObjectKind::Bouncer,
                        render: Render {
                            visual: pl.template.map(|v| [1_010_201, v]),
                            ..Default::default()
                        },
                        source: Some(Source::Surveyed),
                        ..Default::default()
                    },
                );
                tree.bouncers
                    .entry(world.clone())
                    .or_default()
                    .push(Bouncer {
                        id: format!("bouncer/{pf}/{}", pl.id),
                        name: pl.name.clone(),
                        template: Some(template),
                        pos: pl.pos,
                        rot: Some(crate::Rot::Heading(pl.heading)),
                        scale: 1.0,
                        target: pl.target,
                        replaces,
                        removed: pl.removed,
                        comment: pl.comment.clone(),
                        tags: pl.tags.clone(),
                        feature: pl.feature.clone(),
                        author: pl.author.clone(),
                        updated: pl.updated,
                        source: Some(Source::Surveyed),
                        ..Default::default()
                    });
                continue;
            }
            let entry = json!({ "name": pl.name, "monster_data": pl.monster_data, "template": pl.template, "level": pl.level });
            let (kind, template) = match pl.kind {
                sv::Kind::Enemy => (
                    SpawnKind::Enemy,
                    Some(t.npc(tree, ctx, &entry, NpcKind::Enemy, &npc_spells, &npc_life)),
                ),
                sv::Kind::Npc => (
                    SpawnKind::Npc,
                    Some(t.npc(tree, ctx, &entry, NpcKind::Npc, &npc_spells, &npc_life)),
                ),
                k => {
                    let (sk, ok) = match k {
                        sv::Kind::Smashable => (SpawnKind::Smashable, ObjectKind::Smashable),
                        sv::Kind::Buildie => (SpawnKind::Buildie, ObjectKind::Buildie),
                        _ => (SpawnKind::Interactable, ObjectKind::Interactable),
                    };
                    let key = format!(
                        "survey|{ok:?}|{:?}|{:?}|{}",
                        pl.template, pl.monster_data, pl.name
                    );
                    let tpl = t.object(tree, ctx, key, &pl.name, |id| Object {
                        id,
                        name: pl.name.clone(),
                        kind: ok,
                        render: Render {
                            template: pl.template,
                            monster_data: pl.monster_data,
                            ..Default::default()
                        },
                        level: pl.level,
                        loot: (ok == ObjectKind::Smashable).then(|| "loot/smashable".into()),
                        source: Some(Source::Surveyed),
                        ..Default::default()
                    });
                    (sk, Some(tpl))
                }
            };
            let overrides = template
                .as_deref()
                .and_then(|tp| t.overrides(tp, &entry, "stats"));
            tree.spawns.entry(world.clone()).or_default().push(Spawn {
                id: format!("spawn/{pf}/{}", pl.id),
                name: pl.name.clone(),
                kind,
                template,
                pos: pl.pos,
                rot: Some(crate::Rot::Heading(pl.heading)),
                scale: 1.0,
                overrides,
                patrol: pl.patrol.clone(),
                wave: pl.wave.as_ref().and_then(|w| {
                    Some(WaveRef {
                        script: ctx.script.get(&w.script)?.clone(),
                        wave: w.wave.clone(),
                    })
                }),
                replaces,
                removed: pl.removed,
                comment: pl.comment.clone(),
                tags: pl.tags.clone(),
                feature: pl.feature.clone(),
                author: pl.author.clone(),
                updated: pl.updated,
                source: Some(Source::Surveyed),
                ..Default::default()
            });
        }
        for w in &p.triggers {
            let shape = match w.radius {
                Some(radius) => Shape::Circle {
                    pos: w.pos,
                    radius,
                    height: Some(w.size[1]),
                },
                None => Shape::Wall {
                    pos: w.pos,
                    heading: w.heading,
                    size: w.size,
                },
            };
            let action = wall_action(tree, ctx, &w.action);
            tree.triggers
                .entry(world.clone())
                .or_default()
                .push(Trigger {
                    id: format!("trigger/{pf}/{}", w.id),
                    name: w.name.clone(),
                    shape: Some(shape),
                    once: if w.once { Once::PerPlayer } else { Once::Never },
                    look: Some(w.color.clone()),
                    actions: vec![action],
                    removed: w.removed,
                    comment: w.comment.clone(),
                    tags: w.tags.clone(),
                    feature: w.feature.clone(),
                    author: w.author.clone(),
                    updated: w.updated,
                    source: Some(Source::Surveyed),
                    ..Default::default()
                });
        }
    }

    if let Ok(v) = read_json(&dir.join("achievements.json")) {
        let a: sv::AchievementsFile = serde_json::from_value(v)?;
        for r in a.smash {
            let rule = AchievementRule {
                enemies: r.enemies.clone(),
                once: r.once,
                item: r.item,
                comment: r.comment.clone(),
            };
            let lore = r.id.parse::<u32>().ok();
            match lore
                .and_then(|l| ctx.achievement.get(&l))
                .and_then(|id| tree.achievements.iter_mut().find(|x| x.id == *id))
            {
                Some(x) => {
                    x.rule = Some(rule);
                    x.tags.extend(r.tags.iter().cloned());
                }
                None => {
                    let id = ctx.ids.new_id("achievement", &r.id.replace(':', " "));
                    tree.achievements.push(Achievement {
                        id,
                        client_name: Some(r.id.clone()),
                        kind: r.id.split(':').next().unwrap_or("").to_string(),
                        rule: Some(rule),
                        comment: r.comment.clone(),
                        tags: r.tags.clone(),
                        feature: r.feature.clone(),
                        author: r.author.clone(),
                        updated: r.updated,
                        source: Some(Source::Surveyed),
                        ..Default::default()
                    });
                }
            }
        }
    }
    ctx.templates = t;
    Ok(())
}

fn shape(a: &sv::Area) -> Shape {
    match a {
        sv::Area::Point(p) => Shape::Point { pos: *p, rot: None },
        sv::Area::Circle { center, radius } => Shape::Circle {
            pos: *center,
            radius: *radius,
            height: None,
        },
        sv::Area::Polygon { points } => {
            let lo = points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
            let hi = points
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max);
            let (lo, hi) = if lo.is_finite() {
                (lo - 1.0, hi + 4.0)
            } else {
                (0.0, 0.0)
            };
            Shape::Polygon {
                points: points.iter().map(|p| [p[0], p[2]]).collect(),
                heights: [lo, hi],
            }
        }
    }
}

/// The imported spawn a world-editor placement replaces: same kind of thing within 1 m.
fn find_origin(tree: &Tree, world: &str, o: &sv::Origin) -> Option<String> {
    let near = |p: &crate::Vec3| {
        p.iter()
            .zip(o.pos.iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f32>()
            < 1.0
    };
    if o.kind == sv::Kind::Bouncer {
        return tree
            .bouncers
            .get(world)?
            .iter()
            .find(|b| near(&b.pos))
            .map(|b| b.id.clone());
    }
    tree.spawns
        .get(world)?
        .iter()
        .find(|s| near(&s.pos) && s.source != Some(Source::Surveyed))
        .map(|s| s.id.clone())
}

fn wall_action(tree: &Tree, ctx: &Ctx, a: &sv::WallAction) -> Action {
    match a {
        sv::WallAction::Script {
            script: Some(s),
            wave,
        } => match ctx.script.get(s) {
            Some(script) => Action::StartScript {
                script: script.clone(),
                wave: wave.clone(),
            },
            None => Action::Other {
                comment: format!("start script {s} (not found)"),
            },
        },
        sv::WallAction::Script { script: None, .. } => Action::Other {
            comment: "start a script (not chosen yet)".into(),
        },
        sv::WallAction::Discover { name, achievement } => Action::Discover {
            location: name.clone(),
            achievement: achievement
                .as_ref()
                .and_then(|a| a.parse::<u32>().ok())
                .and_then(|l| ctx.achievement.get(&l).cloned()),
        },
        sv::WallAction::Dialog { dialog } => {
            let first: Option<u32> = dialog.as_ref().and_then(|d| d.parse().ok());
            match first.and_then(|f| tree.dialogues.iter().find(|d| d.texts.first() == Some(&f))) {
                Some(d) => Action::Dialogue {
                    dialogue: d.id.clone(),
                },
                None => Action::Other {
                    comment: format!("dialogue {}", dialog.clone().unwrap_or_default()),
                },
            }
        }
        sv::WallAction::Portal {
            window: Some(w), ..
        } => Action::OpenWindow { window: *w },
        sv::WallAction::Portal { window: None, to } => match to.and_then(|t| ctx.world.get(&t)) {
            Some(w) => Action::Teleport {
                to: Destination {
                    world: Some(w.clone()),
                    ..Default::default()
                },
            },
            None => Action::Other {
                comment: format!("portal to {to:?}"),
            },
        },
        sv::WallAction::Message { text } => Action::Message { text: text.clone() },
        sv::WallAction::Collect { drop } => match drop {
            Some(k) => Action::Collect { key: k.clone() },
            None => Action::Other {
                comment: "collect (not chosen yet)".into(),
            },
        },
        sv::WallAction::Other => Action::Other {
            comment: String::new(),
        },
    }
}
