//! The rest of what a 1.8 PvP server shows: titles, the player list's header and footer, scoreboards and
//! teams (sidebars, name colours and prefixes), sounds, block-breaking effects and explosions.
use super::{down, id::clientbound as cb};
use crate::{
    Result, State,
    buf::{Reader, Writer},
    ids::play::clientbound as play,
};

/// 1.8 sends scoreboard and team texts as plain strings (with § colour codes); 1.20.2 as components.
fn component(text: &str) -> String {
    serde_json::json!({ "text": text }).to_string()
}

pub(super) fn clientbound(
    table: &super::chunks::Table,
    id: i32,
    payload: &[u8],
) -> Result<Vec<(i32, Vec<u8>)>> {
    let mut r = Reader::new(payload);
    let mut w = Writer::new();
    Ok(match id {
        cb::TITLE => match r.varint()? {
            0 => vec![(down(State::Play, play::SET_TITLE_TEXT)?, r.rest().to_vec())],
            1 => vec![(
                down(State::Play, play::SET_SUBTITLE_TEXT)?,
                r.rest().to_vec(),
            )],
            // Fade in, stay and fade out, ints in each.
            2 => vec![(
                down(State::Play, play::SET_TITLES_ANIMATION)?,
                r.rest().to_vec(),
            )],
            action @ (3 | 4) => {
                w.bool(action == 4); // reset clears the times too
                vec![(down(State::Play, play::CLEAR_TITLES)?, w.data)]
            }
            _ => Vec::new(),
        },
        // Header and footer, JSON text in each.
        cb::PLAYER_LIST_HEADER_FOOTER => {
            vec![(down(State::Play, play::TAB_LIST)?, payload.to_vec())]
        }
        cb::SCOREBOARD_OBJECTIVE => {
            let name = r.string(16)?;
            let mode = r.i8()?;
            w.string(&name);
            w.i8(mode);
            if mode == 0 || mode == 2 {
                w.string(&component(&r.string(32)?));
                w.varint(i32::from(r.string(16)? == "hearts"));
            }
            vec![(down(State::Play, play::SET_OBJECTIVE)?, w.data)]
        }
        cb::UPDATE_SCORE => {
            let owner = r.string(40)?;
            let action = r.i8()?;
            let objective = r.string(16)?;
            w.string(&owner);
            w.varint(i32::from(action));
            w.string(&objective);
            if action != 1 {
                w.varint(r.varint()?);
            }
            vec![(down(State::Play, play::SET_SCORE)?, w.data)]
        }
        cb::DISPLAY_SCOREBOARD => {
            w.varint(i32::from(r.i8()?));
            w.string(&r.string(16)?);
            vec![(down(State::Play, play::SET_DISPLAY_OBJECTIVE)?, w.data)]
        }
        cb::TEAMS => {
            let name = r.string(16)?;
            let mode = r.i8()?;
            w.string(&name);
            w.i8(mode);
            if mode == 0 || mode == 2 {
                let display = r.string(32)?;
                let prefix = r.string(16)?;
                let suffix = r.string(16)?;
                let options = r.i8()?;
                let visibility = r.string(32)?;
                let color = r.i8()?;
                w.string(&component(&display));
                w.i8(options);
                w.string(&visibility);
                w.string("always"); // collisions: 1.8 had no rule
                // ChatFormatting by id: the 16 colours, then reset (21) for 1.8's -1.
                w.varint(if (0..16).contains(&color) {
                    i32::from(color)
                } else {
                    21
                });
                w.string(&component(&prefix));
                w.string(&component(&suffix));
            }
            if matches!(mode, 0 | 3 | 4) {
                let n = r.count(r.remaining())?;
                w.varint(n as i32);
                for _ in 0..n {
                    w.string(&r.string(40)?);
                }
            }
            vec![(down(State::Play, play::SET_PLAYER_TEAM)?, w.data)]
        }
        cb::SOUND_EFFECT => {
            let name = r.string(256)?;
            let Some(sound) = sound(&name) else {
                return Ok(Vec::new());
            };
            let (x, y, z) = (r.i32()?, r.i32()?, r.i32()?);
            let volume = r.f32()?;
            let pitch = f32::from(r.u8()?) / 63.0;
            w.varint(0); // a sound given by name
            w.string(&sound);
            w.bool(false); // no fixed range
            w.varint(0); // the master volume
            w.i32(x);
            w.i32(y);
            w.i32(z);
            w.f32(volume);
            w.f32(pitch);
            w.i64(0); // seed
            vec![(down(State::Play, play::SOUND)?, w.data)]
        }
        cb::EFFECT => {
            let event = r.i32()?;
            let pos = super::position(r.i64()?);
            let data = r.i32()?;
            let global = r.bool()?;
            let data = match event {
                // A block broke: its number and meta (`id | meta << 12`) in 1.8, a block state in 1.20.2.
                2001 => table[(((data & 0xFFF) << 4) | ((data >> 12) & 15)) as usize & 0xFFF],
                // Smoke and spawner flames keep their numbers; the rest changed since.
                2000 | 2004 => data,
                _ => return Ok(Vec::new()),
            };
            w.i32(event);
            w.i64(pos);
            w.i32(data);
            w.bool(global);
            vec![(down(State::Play, play::LEVEL_EVENT)?, w.data)]
        }
        cb::EXPLOSION => {
            for _ in 0..3 {
                w.f64(f64::from(r.f32()?));
            }
            w.f32(r.f32()?); // power
            let n = r.i32()?.max(0) as usize;
            w.varint(n as i32);
            w.bytes(
                r.bytes(
                    n.checked_mul(3)
                        .ok_or_else(|| crate::Error::new("explosion size"))?,
                )?,
            );
            for _ in 0..3 {
                w.f32(r.f32()?); // the push given to the player
            }
            vec![(down(State::Play, play::EXPLODE)?, w.data)]
        }
        _ => Vec::new(),
    })
}

/// A 1.8 sound name as the modern sound event (1.9 renamed every sound).
fn sound(name: &str) -> Option<String> {
    let fixed = match name {
        "game.player.hurt" => "entity.player.hurt",
        "game.player.die" => "entity.player.death",
        "game.player.hurt.fall.big" => "entity.player.big_fall",
        "game.player.hurt.fall.small" => "entity.player.small_fall",
        "game.player.swim" => "entity.player.swim",
        "game.player.swim.splash" | "liquid.splash" => "entity.player.splash",
        "liquid.swim" => "entity.generic.swim",
        "liquid.water" => "block.water.ambient",
        "liquid.lava" => "block.lava.ambient",
        "liquid.lavapop" => "block.lava.pop",
        "random.bow" => "entity.arrow.shoot",
        "random.bowhit" => "entity.arrow.hit",
        "random.successful_hit" => "entity.arrow.hit_player",
        "random.orb" => "entity.experience_orb.pickup",
        "random.pop" => "entity.item.pickup",
        "random.eat" => "entity.generic.eat",
        "random.drink" => "entity.generic.drink",
        "random.burp" => "entity.player.burp",
        "random.break" => "entity.item.break",
        "random.explode" => "entity.generic.explode",
        "random.levelup" => "entity.player.levelup",
        "random.click" => "ui.button.click",
        "random.wood_click" => "block.wooden_button.click_on",
        "random.door_open" => "block.wooden_door.open",
        "random.door_close" => "block.wooden_door.close",
        "random.chestopen" => "block.chest.open",
        "random.chestclosed" => "block.chest.close",
        "random.fizz" => "block.fire.extinguish",
        "random.fuse" => "entity.tnt.primed",
        "random.splash" => "entity.generic.splash",
        "random.anvil_land" => "block.anvil.land",
        "random.anvil_use" => "block.anvil.use",
        "random.anvil_break" => "block.anvil.destroy",
        "random.glass" => "block.glass.break",
        "fire.fire" => "block.fire.ambient",
        "fire.ignite" => "item.flintandsteel.use",
        "fireworks.launch" => "entity.firework_rocket.launch",
        "fireworks.blast" => "entity.firework_rocket.blast",
        "fireworks.largeBlast" => "entity.firework_rocket.large_blast",
        "fireworks.twinkle" => "entity.firework_rocket.twinkle",
        "tile.piston.out" => "block.piston.extend",
        "tile.piston.in" => "block.piston.contract",
        "portal.portal" => "block.portal.ambient",
        "portal.travel" => "block.portal.travel",
        "portal.trigger" => "block.portal.trigger",
        "ambient.weather.rain" => "weather.rain",
        "ambient.weather.thunder" => "entity.lightning_bolt.thunder",
        "ambient.cave.cave" => "ambient.cave",
        "mob.endermen.portal" => "entity.enderman.teleport",
        "mob.ghast.fireball" => "entity.ghast.shoot",
        "mob.creeper.say" => "entity.creeper.hurt",
        "creeper.primed" => "entity.creeper.primed",
        "note.harp" => "block.note_block.harp",
        "note.bass" | "note.bassattack" => "block.note_block.bass",
        "note.bd" => "block.note_block.basedrum",
        "note.snare" => "block.note_block.snare",
        "note.hat" => "block.note_block.hat",
        "note.pling" => "block.note_block.pling",
        _ => "",
    };
    if !fixed.is_empty() {
        return Some(format!("minecraft:{fixed}"));
    }
    let material = |m: &str| -> Option<&'static str> {
        Some(match m {
            "stone" => "stone",
            "grass" => "grass",
            "wood" => "wood",
            "gravel" => "gravel",
            "sand" => "sand",
            "cloth" => "wool",
            "snow" => "snow",
            "glass" => "glass",
            "ladder" => "ladder",
            _ => return None,
        })
    };
    if let Some(m) = name.strip_prefix("dig.") {
        return material(m).map(|m| format!("minecraft:block.{m}.break"));
    }
    if let Some(m) = name.strip_prefix("step.") {
        return material(m).map(|m| format!("minecraft:block.{m}.step"));
    }
    // mob.<mob>.<say|hurt|death|...>
    let (mob, what) = name.strip_prefix("mob.")?.split_once('.')?;
    let mob = match mob {
        "zombie" | "skeleton" | "spider" | "pig" | "cow" | "sheep" | "chicken" | "wolf"
        | "blaze" | "ghast" | "silverfish" | "witch" | "villager" | "bat" | "guardian"
        | "rabbit" | "endermite" | "slime" | "horse" => mob,
        "endermen" => "enderman",
        "irongolem" => "iron_golem",
        "zombiepig" => "zombified_piglin",
        "magmacube" => "magma_cube",
        "cat" => "cat",
        _ => return None,
    };
    let what = match what {
        "say" | "idle" | "living" | "moan" | "bark" | "meow" | "scream" => "ambient",
        "hurt" | "hit" => "hurt",
        "death" => "death",
        "step" => "step",
        _ => return None,
    };
    Some(format!("minecraft:entity.{mob}.{what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sounds_take_their_modern_names() {
        assert_eq!(
            sound("game.player.hurt").as_deref(),
            Some("minecraft:entity.player.hurt")
        );
        assert_eq!(
            sound("dig.cloth").as_deref(),
            Some("minecraft:block.wool.break")
        );
        assert_eq!(
            sound("mob.zombie.say").as_deref(),
            Some("minecraft:entity.zombie.ambient")
        );
        assert_eq!(
            sound("mob.irongolem.hit").as_deref(),
            Some("minecraft:entity.iron_golem.hurt")
        );
        assert_eq!(sound("no.such.sound"), None);
    }

    #[test]
    fn teams_keep_their_colour_and_members() {
        let mut w = Writer::new();
        w.string("red");
        w.i8(0);
        w.string("Red");
        w.string("§c");
        w.string("");
        w.i8(3);
        w.string("always");
        w.i8(12);
        w.varint(1);
        w.string("Steve");
        let out = clientbound(&[0; 4096], cb::TEAMS, &w.data).unwrap();
        let mut r = Reader::new(&out[0].1);
        assert_eq!((r.string(16).unwrap(), r.i8().unwrap()), ("red".into(), 0));
        assert_eq!(r.string(99).unwrap(), r#"{"text":"Red"}"#);
        assert_eq!(r.i8().unwrap(), 3);
        assert_eq!(r.string(32).unwrap(), "always");
        assert_eq!(r.string(32).unwrap(), "always");
        assert_eq!(r.varint().unwrap(), 12);
        assert_eq!(r.string(99).unwrap(), r#"{"text":"§c"}"#);
        r.string(99).unwrap();
        assert_eq!(
            (r.varint().unwrap(), r.string(40).unwrap()),
            (1, "Steve".into())
        );
    }

    #[test]
    fn titles_split_into_their_packets() {
        let mut w = Writer::new();
        w.varint(0);
        w.string(r#"{"text":"Fight!"}"#);
        let out = clientbound(&[0; 4096], cb::TITLE, &w.data).unwrap();
        assert_eq!(out[0].0, down(State::Play, play::SET_TITLE_TEXT).unwrap());
        assert_eq!(
            Reader::new(&out[0].1).string(99).unwrap(),
            r#"{"text":"Fight!"}"#
        );
    }
}
