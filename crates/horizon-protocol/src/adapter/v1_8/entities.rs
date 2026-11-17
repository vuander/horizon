//! 1.8's entities as 1.20.2's: spawning (players, mobs, objects, experience orbs), movement, removal,
//! metadata, equipment, animations, effects, experience and the player list.
//!
//! 1.8 sends positions as fixed-point numbers (1/32 of a block) and relative moves as bytes of 1/32;
//! 1.20.2 sends doubles and shorts of 1/4096. Entity metadata is rewritten for what every entity, living
//! entity and player has, and for dropped items; the rest of each type's metadata waits for a later slice.
use std::collections::HashMap;

use super::{down, items};
use crate::{
    Result, State,
    buf::{Reader, Writer},
    ids::play::clientbound as play,
};

/// What Horizon needs to know of an entity to rewrite its metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Player,
    Living,
    Item,
    Other,
}

#[derive(Default)]
pub(super) struct Entities {
    kinds: HashMap<i32, Kind>,
}

/// 1.20.2's entity type number by name.
fn entity_type(name: &str) -> Option<i32> {
    super::super::v1_20_2_ids::ENTITY_TYPE_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| i as i32)
}

/// A 1.8 mob type (spawn_mob) by its modern name.
fn mob(kind: u8) -> Option<&'static str> {
    Some(match kind {
        50 => "minecraft:creeper",
        51 => "minecraft:skeleton",
        52 => "minecraft:spider",
        53 => "minecraft:giant",
        54 => "minecraft:zombie",
        55 => "minecraft:slime",
        56 => "minecraft:ghast",
        57 => "minecraft:zombified_piglin",
        58 => "minecraft:enderman",
        59 => "minecraft:cave_spider",
        60 => "minecraft:silverfish",
        61 => "minecraft:blaze",
        62 => "minecraft:magma_cube",
        63 => "minecraft:ender_dragon",
        64 => "minecraft:wither",
        65 => "minecraft:bat",
        66 => "minecraft:witch",
        67 => "minecraft:endermite",
        68 => "minecraft:guardian",
        90 => "minecraft:pig",
        91 => "minecraft:sheep",
        92 => "minecraft:cow",
        93 => "minecraft:chicken",
        94 => "minecraft:squid",
        95 => "minecraft:wolf",
        96 => "minecraft:mooshroom",
        97 => "minecraft:snow_golem",
        98 => "minecraft:ocelot",
        99 => "minecraft:iron_golem",
        100 => "minecraft:horse",
        101 => "minecraft:rabbit",
        120 => "minecraft:villager",
        _ => return None,
    })
}

/// A 1.8 object type (spawn_object, with its data) by its modern name.
fn object(kind: i8, data: i32) -> Option<&'static str> {
    Some(match kind {
        1 => "minecraft:oak_boat",
        2 => "minecraft:item",
        10 => match data {
            1 => "minecraft:chest_minecart",
            2 => "minecraft:furnace_minecart",
            3 => "minecraft:tnt_minecart",
            4 => "minecraft:spawner_minecart",
            5 => "minecraft:hopper_minecart",
            6 => "minecraft:command_block_minecart",
            _ => "minecraft:minecart",
        },
        50 => "minecraft:tnt",
        51 => "minecraft:end_crystal",
        60 => "minecraft:arrow",
        61 => "minecraft:snowball",
        62 => "minecraft:egg",
        63 => "minecraft:fireball",
        64 => "minecraft:small_fireball",
        65 => "minecraft:ender_pearl",
        66 => "minecraft:wither_skull",
        70 => "minecraft:falling_block",
        71 => "minecraft:item_frame",
        72 => "minecraft:eye_of_ender",
        73 => "minecraft:potion",
        75 => "minecraft:experience_bottle",
        76 => "minecraft:firework_rocket",
        77 => "minecraft:leash_knot",
        78 => "minecraft:armor_stand",
        90 => "minecraft:fishing_bobber",
        _ => return None,
    })
}

/// A stand-in UUID for an entity 1.8 sends without one (its objects and mobs), from its network id.
fn uuid_for(entity: i32) -> u128 {
    0x1_8000_0000_0000_4000_8000_0000_0000_u128 | entity as u32 as u128
}

fn fixed(v: i32) -> f64 {
    f64::from(v) / 32.0
}

#[allow(clippy::too_many_arguments)]
fn add_entity(
    entity: i32,
    uuid: u128,
    kind: i32,
    pos: [f64; 3],
    pitch: i8,
    yaw: i8,
    head: i8,
    data: i32,
    velocity: [i16; 3],
) -> Result<(i32, Vec<u8>)> {
    let mut w = Writer::new();
    w.varint(entity);
    w.uuid(uuid);
    w.varint(kind);
    for v in pos {
        w.f64(v);
    }
    w.i8(pitch);
    w.i8(yaw);
    w.i8(head);
    w.varint(data);
    for v in velocity {
        w.i16(v);
    }
    Ok((down(State::Play, play::ADD_ENTITY)?, w.data))
}

/// 1.8's metadata: entries `(type << 5 | index, value)` up to 0x7F.
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Byte(i8),
    Short(i16),
    Int(i32),
    Float(f32),
    String(String),
    Stack(Option<items::Stack>),
    Other,
}

fn read_metadata(r: &mut Reader) -> Result<Vec<(u8, Value)>> {
    let mut out = Vec::new();
    loop {
        let head = r.u8()?;
        if head == 0x7F {
            return Ok(out);
        }
        let index = head & 0x1F;
        let value = match head >> 5 {
            0 => Value::Byte(r.i8()?),
            1 => Value::Short(r.i16()?),
            2 => Value::Int(r.i32()?),
            3 => Value::Float(r.f32()?),
            4 => Value::String(r.string(32767)?),
            5 => Value::Stack(items::read_stack(r)?),
            6 => {
                r.bytes(12)?; // x, y, z ints
                Value::Other
            }
            _ => {
                r.bytes(12)?; // three floats
                Value::Other
            }
        };
        out.push((index, value));
    }
}

/// 1.20.2's entity data serializers used here.
mod serializer {
    pub const BYTE: i32 = 0;
    pub const INT: i32 = 1;
    pub const FLOAT: i32 = 3;
    pub const OPTIONAL_COMPONENT: i32 = 6;
    pub const ITEM_STACK: i32 = 7;
    pub const BOOLEAN: i32 = 8;
    pub const POSE: i32 = 20;
}

/// 1.8 metadata as 1.20.2 entity data, for the entity kind; empty when nothing carries over.
fn metadata(kind: Kind, entries: &[(u8, Value)]) -> Vec<u8> {
    use serializer as s;
    let mut w = Writer::new();
    let mut any = false;
    let mut put = |w: &mut Writer, index: u8, serializer: i32| {
        any = true;
        w.u8(index);
        w.varint(serializer);
    };
    let living = matches!(kind, Kind::Player | Kind::Living);
    for (index, value) in entries {
        match (*index, value) {
            (0, Value::Byte(flags)) => {
                // 1.8's 0x10 was using an item (eating, drinking, blocking); 1.20.2 keeps that apart.
                put(&mut w, 0, s::BYTE);
                w.i8(flags & !0x10);
                if living {
                    put(&mut w, 8, s::BYTE);
                    w.i8(i8::from(flags & 0x10 != 0));
                }
                if kind == Kind::Player {
                    put(&mut w, 6, s::POSE);
                    w.varint(if flags & 0x02 != 0 { 5 } else { 0 }); // crouching or standing
                }
            }
            (1, Value::Short(air)) => {
                put(&mut w, 1, s::INT);
                w.varint(i32::from(*air));
            }
            (2, Value::String(name)) => {
                put(&mut w, 2, s::OPTIONAL_COMPONENT);
                w.bool(!name.is_empty());
                if !name.is_empty() {
                    w.string(&serde_json::json!({ "text": name }).to_string());
                }
            }
            (3, Value::Byte(shown)) => {
                put(&mut w, 3, s::BOOLEAN);
                w.bool(*shown != 0);
            }
            (4, Value::Byte(silent)) => {
                put(&mut w, 4, s::BOOLEAN);
                w.bool(*silent != 0);
            }
            (6, Value::Float(health)) if living => {
                put(&mut w, 9, s::FLOAT);
                w.f32(*health);
            }
            (7, Value::Int(color)) if living => {
                put(&mut w, 10, s::INT);
                w.varint(*color);
            }
            (8, Value::Byte(ambient)) if living => {
                put(&mut w, 11, s::BOOLEAN);
                w.bool(*ambient != 0);
            }
            (9, Value::Byte(arrows)) if living => {
                put(&mut w, 12, s::INT);
                w.varint(i32::from(*arrows));
            }
            (12, Value::Byte(age)) if kind == Kind::Living => {
                put(&mut w, 16, s::BOOLEAN);
                w.bool(*age < 0); // a baby
            }
            (10, Value::Byte(skin)) if kind == Kind::Player => {
                put(&mut w, 17, s::BYTE);
                w.i8(*skin);
            }
            (17, Value::Float(absorption)) if kind == Kind::Player => {
                put(&mut w, 15, s::FLOAT);
                w.f32(*absorption);
            }
            (18, Value::Int(score)) if kind == Kind::Player => {
                put(&mut w, 16, s::INT);
                w.varint(*score);
            }
            (10, Value::Stack(stack)) if kind == Kind::Item => {
                put(&mut w, 8, s::ITEM_STACK);
                items::write_item(&mut w, stack.as_ref());
            }
            _ => {}
        }
    }
    if !any {
        return Vec::new();
    }
    w.u8(0xFF);
    w.data
}

fn entity_data(entity: i32, kind: Kind, entries: &[(u8, Value)]) -> Result<Option<(i32, Vec<u8>)>> {
    let data = metadata(kind, entries);
    if data.is_empty() {
        return Ok(None);
    }
    let mut w = Writer::new();
    w.varint(entity);
    w.bytes(&data);
    Ok(Some((down(State::Play, play::SET_ENTITY_DATA)?, w.data)))
}

/// 1.8's relative moves are bytes of 1/32 of a block; 1.20.2's shorts of 1/4096.
fn delta(r: &mut Reader) -> Result<i16> {
    Ok(i16::from(r.i8()?) * 128)
}

pub(super) fn clientbound(
    entities: &mut Entities,
    table: &super::chunks::Table,
    id: i32,
    payload: &[u8],
) -> Result<Vec<(i32, Vec<u8>)>> {
    use super::id::clientbound as cb;
    let mut r = Reader::new(payload);
    let mut w = Writer::new();
    Ok(match id {
        cb::SPAWN_PLAYER => {
            let entity = r.varint()?;
            let uuid = r.uuid()?;
            let pos = [fixed(r.i32()?), fixed(r.i32()?), fixed(r.i32()?)];
            let yaw = r.i8()?;
            let pitch = r.i8()?;
            let _item = r.i16()?;
            let meta = read_metadata(&mut r)?;
            entities.kinds.insert(entity, Kind::Player);
            let Some(player) = entity_type("minecraft:player") else {
                return Ok(Vec::new());
            };
            let mut out = vec![add_entity(
                entity, uuid, player, pos, pitch, yaw, yaw, 0, [0; 3],
            )?];
            out.extend(entity_data(entity, Kind::Player, &meta)?);
            out
        }
        cb::SPAWN_OBJECT => {
            let entity = r.varint()?;
            let kind = r.i8()?;
            let pos = [fixed(r.i32()?), fixed(r.i32()?), fixed(r.i32()?)];
            let pitch = r.i8()?;
            let yaw = r.i8()?;
            let data = r.i32()?;
            let velocity = if data != 0 {
                [r.i16()?, r.i16()?, r.i16()?]
            } else {
                [0; 3]
            };
            let Some(type_id) = object(kind, data).and_then(entity_type) else {
                return Ok(Vec::new());
            };
            let data = match kind {
                // The shooter's id plus one in 1.8, the owner's id in 1.20.2.
                60 | 90 => (data - 1).max(0),
                // A block number and meta (`id | meta << 12`) in 1.8, a block state in 1.20.2.
                70 => table[(((data & 0xFFF) << 4) | ((data >> 12) & 15)) as usize & 0xFFF],
                10 => 0,
                _ => data,
            };
            entities
                .kinds
                .insert(entity, if kind == 2 { Kind::Item } else { Kind::Other });
            vec![add_entity(
                entity,
                uuid_for(entity),
                type_id,
                pos,
                pitch,
                yaw,
                yaw,
                data,
                velocity,
            )?]
        }
        cb::SPAWN_MOB => {
            let entity = r.varint()?;
            let kind = r.u8()?;
            let pos = [fixed(r.i32()?), fixed(r.i32()?), fixed(r.i32()?)];
            let yaw = r.i8()?;
            let pitch = r.i8()?;
            let head = r.i8()?;
            let velocity = [r.i16()?, r.i16()?, r.i16()?];
            let meta = read_metadata(&mut r)?;
            let Some(type_id) = mob(kind).and_then(entity_type) else {
                return Ok(Vec::new());
            };
            entities.kinds.insert(entity, Kind::Living);
            let mut out = vec![add_entity(
                entity,
                uuid_for(entity),
                type_id,
                pos,
                pitch,
                yaw,
                head,
                0,
                velocity,
            )?];
            out.extend(entity_data(entity, Kind::Living, &meta)?);
            out
        }
        cb::SPAWN_EXPERIENCE_ORB => {
            w.varint(r.varint()?);
            for _ in 0..3 {
                w.f64(fixed(r.i32()?));
            }
            w.i16(r.i16()?);
            // 1.20.2's add_experience_orb, which the pinned version folded into add_entity.
            vec![(ADD_EXPERIENCE_ORB_1_20_2, w.data)]
        }
        // Entity id and three shorts of velocity in each.
        cb::ENTITY_VELOCITY => vec![(
            down(State::Play, play::SET_ENTITY_MOTION)?,
            payload.to_vec(),
        )],
        // A count and the ids, as VarInts in each.
        cb::DESTROY_ENTITIES => {
            let n = r.count(r.remaining())?;
            for _ in 0..n {
                entities.kinds.remove(&r.varint()?);
            }
            vec![(down(State::Play, play::REMOVE_ENTITIES)?, payload.to_vec())]
        }
        cb::ENTITY_RELATIVE_MOVE => {
            w.varint(r.varint()?);
            for _ in 0..3 {
                w.i16(delta(&mut r)?);
            }
            w.bool(r.bool()?);
            vec![(down(State::Play, play::MOVE_ENTITY_POS)?, w.data)]
        }
        // Entity, yaw, pitch and ground flag in each.
        cb::ENTITY_LOOK => vec![(down(State::Play, play::MOVE_ENTITY_ROT)?, payload.to_vec())],
        cb::ENTITY_LOOK_AND_RELATIVE_MOVE => {
            w.varint(r.varint()?);
            for _ in 0..3 {
                w.i16(delta(&mut r)?);
            }
            w.i8(r.i8()?);
            w.i8(r.i8()?);
            w.bool(r.bool()?);
            vec![(down(State::Play, play::MOVE_ENTITY_POS_ROT)?, w.data)]
        }
        cb::ENTITY_TELEPORT => {
            w.varint(r.varint()?);
            for _ in 0..3 {
                w.f64(fixed(r.i32()?));
            }
            w.i8(r.i8()?);
            w.i8(r.i8()?);
            w.bool(r.bool()?);
            vec![(down(State::Play, play::TELEPORT_ENTITY)?, w.data)]
        }
        // Entity and head yaw in each.
        cb::ENTITY_HEAD_LOOK => vec![(down(State::Play, play::ROTATE_HEAD)?, payload.to_vec())],
        cb::ENTITY_STATUS => {
            let entity = r.i32()?;
            let status = r.i8()?;
            if status == 2 {
                // Hurt: since 1.19.4 its own packet, with the direction the hit came from.
                w.varint(entity);
                w.f32(0.0);
                vec![(down(State::Play, play::HURT_ANIMATION)?, w.data)]
            } else {
                vec![(down(State::Play, play::ENTITY_EVENT)?, payload.to_vec())]
            }
        }
        cb::ENTITY_METADATA => {
            let entity = r.varint()?;
            let meta = read_metadata(&mut r)?;
            let kind = entities.kinds.get(&entity).copied().unwrap_or(Kind::Other);
            entity_data(entity, kind, &meta)?.into_iter().collect()
        }
        cb::ENTITY_EQUIPMENT => {
            let entity = r.varint()?;
            let slot = r.i16()?;
            let stack = items::read_stack(&mut r)?;
            // 1.8: held, boots, leggings, chestplate, helmet; 1.20.2: main hand, off hand, feet, legs,
            // chest, head.
            let slot = match slot {
                0 => 0,
                1..=4 => slot as u8 + 1,
                _ => return Ok(Vec::new()),
            };
            w.varint(entity);
            w.u8(slot);
            items::write_item(&mut w, stack.as_ref());
            vec![(down(State::Play, play::SET_EQUIPMENT)?, w.data)]
        }
        cb::ANIMATION => {
            let entity = r.varint()?;
            match r.u8()? {
                // Swing, wake up, critical and magic critical hits keep their numbers.
                action @ (0 | 2 | 4 | 5) => {
                    w.varint(entity);
                    w.u8(action);
                    vec![(down(State::Play, play::ANIMATE)?, w.data)]
                }
                1 => {
                    w.varint(entity);
                    w.f32(0.0);
                    vec![(down(State::Play, play::HURT_ANIMATION)?, w.data)]
                }
                _ => Vec::new(),
            }
        }
        cb::COLLECT_ITEM => {
            w.varint(r.varint()?);
            w.varint(r.varint()?);
            w.varint(1); // 1.8 doesn't say how many
            vec![(down(State::Play, play::TAKE_ITEM_ENTITY)?, w.data)]
        }
        cb::ENTITY_EFFECT => {
            let entity = r.varint()?;
            let effect = i32::from(r.i8()?);
            let amplifier = r.i8()?;
            let duration = r.varint()?;
            let hide = r.bool()?;
            w.varint(entity);
            w.varint(effect - 1); // 1.8 counts from 1, 1.20.2 from 0
            w.i8(amplifier);
            w.varint(duration);
            w.u8(if hide { 0x4 } else { 0x2 | 0x4 }); // particles visible, icon shown
            w.bool(false); // no factor data
            vec![(down(State::Play, play::UPDATE_MOB_EFFECT)?, w.data)]
        }
        cb::REMOVE_ENTITY_EFFECT => {
            w.varint(r.varint()?);
            w.varint(i32::from(r.i8()?) - 1);
            vec![(down(State::Play, play::REMOVE_MOB_EFFECT)?, w.data)]
        }
        // Bar, level and total experience in each.
        cb::SET_EXPERIENCE => vec![(down(State::Play, play::SET_EXPERIENCE)?, payload.to_vec())],
        cb::PLAYER_LIST_ITEM => player_list(&mut r)?,
        _ => Vec::new(),
    })
}

/// 1.20.2's add_experience_orb, which has no pinned id ([`crate::adapter::Session`] reads it as such).
const ADD_EXPERIENCE_ORB_1_20_2: i32 = 2;

/// 1.8's player_list_item (one action for every entry) as 1.20.2's player_info_update or _remove.
fn player_list(r: &mut Reader) -> Result<Vec<(i32, Vec<u8>)>> {
    // 1.20.2's actions: add player 0x01, initialize chat 0x02, game mode 0x04, listed 0x08,
    // latency 0x10, display name 0x20.
    let action = r.varint()?;
    let n = r.count(r.remaining() / 16)?;
    let mut w = Writer::new();
    if action == 4 {
        w.varint(n as i32);
        for _ in 0..n {
            w.uuid(r.uuid()?);
        }
        return Ok(vec![(down(State::Play, play::PLAYER_INFO_REMOVE)?, w.data)]);
    }
    let actions: u8 = match action {
        0 => 0x01 | 0x04 | 0x08 | 0x10 | 0x20,
        1 => 0x04,
        2 => 0x10,
        3 => 0x20,
        _ => return Ok(Vec::new()),
    };
    w.u8(actions);
    w.varint(n as i32);
    let display_name = |r: &mut Reader, w: &mut Writer| -> Result<()> {
        let has = r.bool()?;
        w.bool(has);
        if has {
            w.string(&r.string(32767)?);
        }
        Ok(())
    };
    for _ in 0..n {
        w.uuid(r.uuid()?);
        match action {
            0 => {
                w.string(&r.string(16)?);
                let properties = r.count(r.remaining())?;
                w.varint(properties as i32);
                for _ in 0..properties {
                    w.string(&r.string(32767)?);
                    w.string(&r.string(32767)?);
                    let signed = r.bool()?;
                    w.bool(signed);
                    if signed {
                        w.string(&r.string(32767)?);
                    }
                }
                w.varint(r.varint()?); // game mode
                w.bool(true); // listed
                w.varint(r.varint()?); // latency
                display_name(r, &mut w)?;
            }
            1 | 2 => w.varint(r.varint()?),
            _ => display_name(r, &mut w)?,
        }
    }
    Ok(vec![(down(State::Play, play::PLAYER_INFO_UPDATE)?, w.data)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn players_spawn_with_their_metadata() {
        let mut entities = Entities::default();
        let table = super::super::chunks::tests::test_table();
        let mut w = Writer::new();
        w.varint(7);
        w.uuid(0xABCD);
        w.i32(32 * 10);
        w.i32(32 * 64 + 16);
        w.i32(-32 * 3);
        w.i8(64);
        w.i8(0);
        w.i16(0);
        w.u8(0); // byte, index 0
        w.i8(0x02 | 0x10); // crouching, blocking
        w.u8(3 << 5 | 6); // float, index 6
        w.f32(17.5);
        w.u8(0x7F);
        let out = clientbound(
            &mut entities,
            &table,
            super::super::id::clientbound::SPAWN_PLAYER,
            &w.data,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        let mut r = Reader::new(&out[0].1);
        assert_eq!(r.varint().unwrap(), 7);
        assert_eq!(r.uuid().unwrap(), 0xABCD);
        assert_eq!(
            r.varint().unwrap(),
            entity_type("minecraft:player").unwrap()
        );
        assert_eq!(
            [r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap()],
            [10.0, 64.5, -3.0]
        );
        // Entity data: flags without 0x10, the using-item flag, the crouching pose, then health.
        let mut r = Reader::new(&out[1].1);
        assert_eq!(r.varint().unwrap(), 7);
        assert_eq!(
            (r.u8().unwrap(), r.varint().unwrap(), r.i8().unwrap()),
            (0, 0, 0x02)
        );
        assert_eq!(
            (r.u8().unwrap(), r.varint().unwrap(), r.i8().unwrap()),
            (8, 0, 1)
        );
        assert_eq!(
            (r.u8().unwrap(), r.varint().unwrap(), r.varint().unwrap()),
            (6, 20, 5)
        );
        assert_eq!(
            (r.u8().unwrap(), r.varint().unwrap(), r.f32().unwrap()),
            (9, 3, 17.5)
        );
        assert_eq!(r.u8().unwrap(), 0xFF);
    }

    #[test]
    fn relative_moves_scale_to_4096ths() {
        let mut w = Writer::new();
        w.varint(3);
        w.i8(16); // half a block
        w.i8(-32);
        w.i8(0);
        w.bool(true);
        let out = clientbound(
            &mut Entities::default(),
            &super::super::chunks::tests::test_table(),
            super::super::id::clientbound::ENTITY_RELATIVE_MOVE,
            &w.data,
        )
        .unwrap();
        let mut r = Reader::new(&out[0].1);
        assert_eq!(r.varint().unwrap(), 3);
        assert_eq!(
            [r.i16().unwrap(), r.i16().unwrap(), r.i16().unwrap()],
            [2048, -4096, 0]
        );
        assert!(r.bool().unwrap());
    }

    #[test]
    fn the_player_list_adds_with_skins_and_removes() {
        let mut w = Writer::new();
        w.varint(0);
        w.varint(1);
        w.uuid(42);
        w.string("Notch");
        w.varint(1);
        w.string("textures");
        w.string("base64");
        w.bool(true);
        w.string("signature");
        w.varint(0);
        w.varint(35);
        w.bool(false);
        let out = player_list(&mut Reader::new(&w.data)).unwrap();
        let mut r = Reader::new(&out[0].1);
        assert_eq!(r.u8().unwrap(), 0x3D);
        assert_eq!(r.varint().unwrap(), 1);
        assert_eq!(r.uuid().unwrap(), 42);
        assert_eq!(r.string(16).unwrap(), "Notch");
        let mut w = Writer::new();
        w.varint(4);
        w.varint(1);
        w.uuid(42);
        let out = player_list(&mut Reader::new(&w.data)).unwrap();
        assert_eq!(
            out[0].0,
            down(State::Play, play::PLAYER_INFO_REMOVE).unwrap()
        );
    }

    #[test]
    fn hurt_becomes_the_hurt_animation() {
        let mut w = Writer::new();
        w.i32(9);
        w.i8(2);
        let out = clientbound(
            &mut Entities::default(),
            &super::super::chunks::tests::test_table(),
            super::super::id::clientbound::ENTITY_STATUS,
            &w.data,
        )
        .unwrap();
        assert_eq!(out[0].0, down(State::Play, play::HURT_ANIMATION).unwrap());
    }
}
