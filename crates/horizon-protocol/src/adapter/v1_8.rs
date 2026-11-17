//! A 1.8.x server (protocol 47): the oldest step of the adapter (roadmap `older-servers`).
//!
//! 1.8 has no configuration state and numbers and lays out nearly every packet differently, so this step
//! does not follow the release-by-release chain packet by packet. A 1.8 packet becomes the packets a
//! 1.20.2 server would have sent for it, which then go up the shared steps like a 1.20.2 server's; on the
//! way back, Horizon's packets come down those steps to 1.20.2 and are rewritten here as 1.8 packets.
//!
//! The first slice: status, login (offline and online), the configuration 1.8 lacks (synthesised when the
//! server joins the player to its world), and play's keep-alive, join, position, chat, time, health,
//! abilities, difficulty, respawn and disconnect; chunks and block changes ([`chunks`]). Every other 1.8
//! packet is dropped until a later slice reads it: its id means something else in 1.20.2.
use std::sync::Mutex;

use super::ServerVersion;
use crate::{
    Error, Result, State,
    buf::{Reader, Writer},
    ids,
};
use horizon_core::nbt::Tag;

mod chunks;
mod entities;
mod items;
mod world;

pub const PROTOCOL: i32 = 47;
pub const NAME: &str = "1.8.9";

/// 1.8's packet ids by state, as wiki.vg's protocol 47 page numbers them.
mod id {
    pub mod login {
        pub const DISCONNECT: i32 = 0x00;
        pub const ENCRYPTION_REQUEST: i32 = 0x01;
        pub const LOGIN_SUCCESS: i32 = 0x02;
        pub const SET_COMPRESSION: i32 = 0x03;
        pub const LOGIN_START: i32 = 0x00;
        pub const ENCRYPTION_RESPONSE: i32 = 0x01;
    }
    pub mod clientbound {
        pub const KEEP_ALIVE: i32 = 0x00;
        pub const JOIN_GAME: i32 = 0x01;
        pub const CHAT: i32 = 0x02;
        pub const TIME_UPDATE: i32 = 0x03;
        pub const ENTITY_EQUIPMENT: i32 = 0x04;
        pub const SPAWN_POSITION: i32 = 0x05;
        pub const UPDATE_HEALTH: i32 = 0x06;
        pub const RESPAWN: i32 = 0x07;
        pub const POSITION_AND_LOOK: i32 = 0x08;
        pub const HELD_ITEM_CHANGE: i32 = 0x09;
        pub const ANIMATION: i32 = 0x0B;
        pub const SPAWN_PLAYER: i32 = 0x0C;
        pub const COLLECT_ITEM: i32 = 0x0D;
        pub const SPAWN_OBJECT: i32 = 0x0E;
        pub const SPAWN_MOB: i32 = 0x0F;
        pub const SPAWN_EXPERIENCE_ORB: i32 = 0x11;
        pub const ENTITY_VELOCITY: i32 = 0x12;
        pub const DESTROY_ENTITIES: i32 = 0x13;
        pub const ENTITY_RELATIVE_MOVE: i32 = 0x15;
        pub const ENTITY_LOOK: i32 = 0x16;
        pub const ENTITY_LOOK_AND_RELATIVE_MOVE: i32 = 0x17;
        pub const ENTITY_TELEPORT: i32 = 0x18;
        pub const ENTITY_HEAD_LOOK: i32 = 0x19;
        pub const ENTITY_STATUS: i32 = 0x1A;
        pub const ENTITY_METADATA: i32 = 0x1C;
        pub const ENTITY_EFFECT: i32 = 0x1D;
        pub const REMOVE_ENTITY_EFFECT: i32 = 0x1E;
        pub const SET_EXPERIENCE: i32 = 0x1F;
        pub const CHUNK_DATA: i32 = 0x21;
        pub const MULTI_BLOCK_CHANGE: i32 = 0x22;
        pub const BLOCK_CHANGE: i32 = 0x23;
        pub const MAP_CHUNK_BULK: i32 = 0x26;
        pub const EXPLOSION: i32 = 0x27;
        pub const EFFECT: i32 = 0x28;
        pub const SOUND_EFFECT: i32 = 0x29;
        pub const CHANGE_GAME_STATE: i32 = 0x2B;
        pub const OPEN_WINDOW: i32 = 0x2D;
        pub const CLOSE_WINDOW: i32 = 0x2E;
        pub const SET_SLOT: i32 = 0x2F;
        pub const WINDOW_ITEMS: i32 = 0x30;
        pub const WINDOW_PROPERTY: i32 = 0x31;
        pub const CONFIRM_TRANSACTION: i32 = 0x32;
        pub const PLAYER_LIST_ITEM: i32 = 0x38;
        pub const SCOREBOARD_OBJECTIVE: i32 = 0x3B;
        pub const UPDATE_SCORE: i32 = 0x3C;
        pub const DISPLAY_SCOREBOARD: i32 = 0x3D;
        pub const TEAMS: i32 = 0x3E;
        pub const PLAYER_ABILITIES: i32 = 0x39;
        pub const PLUGIN_MESSAGE: i32 = 0x3F;
        pub const DISCONNECT: i32 = 0x40;
        pub const SERVER_DIFFICULTY: i32 = 0x41;
        pub const TITLE: i32 = 0x45;
        pub const PLAYER_LIST_HEADER_FOOTER: i32 = 0x47;
    }
    pub mod serverbound {
        pub const KEEP_ALIVE: i32 = 0x00;
        pub const CHAT: i32 = 0x01;
        pub const USE_ENTITY: i32 = 0x02;
        pub const PLAYER: i32 = 0x03;
        pub const POSITION: i32 = 0x04;
        pub const LOOK: i32 = 0x05;
        pub const POSITION_AND_LOOK: i32 = 0x06;
        pub const PLAYER_DIGGING: i32 = 0x07;
        pub const BLOCK_PLACEMENT: i32 = 0x08;
        pub const HELD_ITEM_CHANGE: i32 = 0x09;
        pub const ANIMATION: i32 = 0x0A;
        pub const ENTITY_ACTION: i32 = 0x0B;
        pub const CLOSE_WINDOW: i32 = 0x0D;
        pub const CLICK_WINDOW: i32 = 0x0E;
        pub const CONFIRM_TRANSACTION: i32 = 0x0F;
        pub const PLAYER_ABILITIES: i32 = 0x13;
        pub const CLIENT_SETTINGS: i32 = 0x15;
        pub const CLIENT_STATUS: i32 = 0x16;
        pub const PLUGIN_MESSAGE: i32 = 0x17;
    }
}

/// 1.20.2's serverbound play ids matched here by number: the shared steps make some of them of 26.3
/// packets with no id of their own (26.3's punch becomes 1.20.2's swing).
mod sb_1_20_2 {
    pub const CONTAINER_CLICK: i32 = 13;
    pub const CONTAINER_CLOSE: i32 = 14;
    pub const INTERACT: i32 = 18;
    pub const PLAYER_ABILITIES: i32 = 31;
    pub const PLAYER_ACTION: i32 = 32;
    pub const PLAYER_COMMAND: i32 = 33;
    pub const SWING: i32 = 50;
    pub const USE_ITEM_ON: i32 = 52;
    pub const USE_ITEM: i32 = 53;
}

/// The longest chat message a 1.8 server accepts (`C01PacketChatMessage`).
const MAX_CHAT: usize = 100;

/// What one 1.8 connection remembers between packets.
#[derive(Default)]
pub(super) struct Legacy {
    /// The server switched to play at login success; Horizon is still configuring until the join.
    server_in_play: bool,
    /// 1.8 teleports carry no id; Horizon's confirmations of the ids given here are not sent.
    next_teleport: i32,
    /// Client settings from configuration, sent with Horizon's first play packet.
    settings: Option<Vec<u8>>,
    /// The player's dimension, as 1.8 numbers it (-1 nether, 0 overworld, 1 end).
    dimension: i8,
    /// The player's x and z, followed from teleports and Horizon's moves.
    position: Option<[f64; 2]>,
    /// The chunk Horizon was last told to keep chunks around ([`center`]).
    center: Option<[i32; 2]>,
    entities: entities::Entities,
    inventory: items::Inventory,
    /// Horizon's block actions to acknowledge (their sequence numbers): 1.8 has no acknowledgement, and
    /// Horizon keeps its own guess of a block until one comes.
    acks: Vec<i32>,
}

/// 1.8 servers never say which chunk the player is in; Horizon only keeps chunks within its view of that
/// one. Its `set_chunk_cache_center` when the player's chunk changed, or for the first position.
fn center(state: &mut Legacy) -> Result<Option<(i32, Vec<u8>)>> {
    let Some([x, z]) = state.position else {
        return Ok(None);
    };
    let chunk = [(x.floor() as i32) >> 4, (z.floor() as i32) >> 4];
    if state.center == Some(chunk) {
        return Ok(None);
    }
    state.center = Some(chunk);
    let mut w = Writer::new();
    w.varint(chunk[0]);
    w.varint(chunk[1]);
    Ok(Some((
        down(State::Play, ids::play::clientbound::SET_CHUNK_CACHE_CENTER)?,
        w.data,
    )))
}

pub(super) type Shared = Mutex<Legacy>;

fn body(id: i32, payload: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.varint(id);
    w.bytes(payload);
    w.data
}

/// 1.20.2's id for a pinned clientbound packet.
fn down(state: State, pinned: i32) -> Result<i32> {
    ServerVersion::V1_20_2
        .clientbound_id_down(state, pinned)
        .ok_or_else(|| Error::new("no 1.20.2 packet for a 1.8 one"))
}

/// A server's packet as the 1.20.2 packets (`state`'s numbering at the time each is read, in order)
/// that stand for it. Empty drops it.
pub(super) fn clientbound(
    legacy: &Shared,
    reading: State,
    id: i32,
    payload: &[u8],
) -> Result<Vec<(i32, Vec<u8>)>> {
    use ids::login::clientbound as login;
    use ids::play::clientbound as play;
    let mut state = legacy.lock().unwrap();
    if reading == State::Status {
        return Ok(vec![(id, payload.to_vec())]);
    }
    if !state.server_in_play {
        return Ok(match id {
            id::login::DISCONNECT => vec![(
                down(State::Login, login::LOGIN_DISCONNECT)?,
                payload.to_vec(),
            )],
            // 1.20.2's hello is 1.8's: server id, key and token, both arrays VarInt-prefixed.
            id::login::ENCRYPTION_REQUEST => {
                vec![(down(State::Login, login::HELLO)?, payload.to_vec())]
            }
            id::login::SET_COMPRESSION => {
                vec![(
                    down(State::Login, login::LOGIN_COMPRESSION)?,
                    payload.to_vec(),
                )]
            }
            id::login::LOGIN_SUCCESS => {
                state.server_in_play = true;
                vec![(
                    down(State::Login, login::LOGIN_FINISHED)?,
                    login_success(payload)?,
                )]
            }
            _ => Vec::new(),
        });
    }
    use id::clientbound as cb;
    // A move Horizon sent since the last packet may have crossed into another chunk.
    let mut out: Vec<(i32, Vec<u8>)> = center(&mut state)?.into_iter().collect();
    for sequence in std::mem::take(&mut state.acks) {
        let mut w = Writer::new();
        w.varint(sequence);
        out.push((down(State::Play, play::BLOCK_CHANGED_ACK)?, w.data));
    }
    out.extend(match id {
        cb::KEEP_ALIVE => {
            let mut w = Writer::new();
            w.i64(i64::from(Reader::new(payload).varint()?));
            vec![(down(State::Play, play::KEEP_ALIVE)?, w.data)]
        }
        cb::JOIN_GAME => {
            let (dimension, packets) = join_game(payload)?;
            state.dimension = dimension;
            packets
        }
        cb::RESPAWN => {
            let (dimension, packets) = respawn(payload)?;
            state.dimension = dimension;
            packets
        }
        cb::CHUNK_DATA | cb::MAP_CHUNK_BULK | cb::BLOCK_CHANGE | cb::MULTI_BLOCK_CHANGE => {
            chunks::clientbound(chunks::table(), state.dimension, id, payload)?
        }
        cb::ENTITY_EQUIPMENT
        | cb::ANIMATION
        | cb::SPAWN_PLAYER
        | cb::COLLECT_ITEM
        | cb::SPAWN_OBJECT
        | cb::SPAWN_MOB
        | cb::SPAWN_EXPERIENCE_ORB
        | cb::ENTITY_VELOCITY
        | cb::DESTROY_ENTITIES
        | cb::ENTITY_RELATIVE_MOVE
        | cb::ENTITY_LOOK
        | cb::ENTITY_LOOK_AND_RELATIVE_MOVE
        | cb::ENTITY_TELEPORT
        | cb::ENTITY_HEAD_LOOK
        | cb::ENTITY_STATUS
        | cb::ENTITY_METADATA
        | cb::ENTITY_EFFECT
        | cb::REMOVE_ENTITY_EFFECT
        | cb::SET_EXPERIENCE
        | cb::PLAYER_LIST_ITEM => {
            entities::clientbound(&mut state.entities, chunks::table(), id, payload)?
        }
        cb::OPEN_WINDOW
        | cb::CLOSE_WINDOW
        | cb::SET_SLOT
        | cb::WINDOW_ITEMS
        | cb::WINDOW_PROPERTY
        | cb::CONFIRM_TRANSACTION => items::clientbound(&mut state.inventory, id, payload)?,
        cb::EXPLOSION
        | cb::EFFECT
        | cb::SOUND_EFFECT
        | cb::SCOREBOARD_OBJECTIVE
        | cb::UPDATE_SCORE
        | cb::DISPLAY_SCOREBOARD
        | cb::TEAMS
        | cb::TITLE
        | cb::PLAYER_LIST_HEADER_FOOTER => world::clientbound(chunks::table(), id, payload)?,
        cb::CHAT => {
            let mut r = Reader::new(payload);
            let json = r.string(32767)?;
            let position = r.i8()?;
            let mut w = Writer::new();
            w.string(&json);
            w.bool(position == 2); // 2: above the hotbar
            vec![(down(State::Play, play::SYSTEM_CHAT)?, w.data)]
        }
        // World age and time of day, both longs in each.
        cb::TIME_UPDATE => vec![(down(State::Play, play::SET_TIME)?, payload.to_vec())],
        cb::SPAWN_POSITION => {
            let packed = position(Reader::new(payload).i64()?);
            let mut w = Writer::new();
            w.i64(packed);
            w.f32(0.0);
            let mut packets = vec![(down(State::Play, play::SET_DEFAULT_SPAWN_POSITION)?, w.data)];
            // Chunks may come before the first teleport: centre them on the spawn until then.
            if state.position.is_none() {
                let [x, _, z] = crate::buf::unpack_block_pos(packed);
                state.position = Some([f64::from(x), f64::from(z)]);
                packets.extend(center(&mut state)?);
            }
            packets
        }
        // Health, food (VarInt) and saturation in each.
        cb::UPDATE_HEALTH => vec![(down(State::Play, play::SET_HEALTH)?, payload.to_vec())],
        cb::POSITION_AND_LOOK => {
            let mut r = Reader::new(payload);
            let (x, _, z) = (r.f64()?, r.f64()?, r.f64()?);
            let (_, _, flags) = (r.f32()?, r.f32()?, r.i8()?);
            let [old_x, old_z] = state.position.unwrap_or_default();
            state.position = Some([
                if flags & 0x1 != 0 { old_x + x } else { x },
                if flags & 0x4 != 0 { old_z + z } else { z },
            ]);
            let mut packets: Vec<_> = center(&mut state)?.into_iter().collect();
            // Position, rotation and relative flags as 1.20.2 lays them out, without its teleport id.
            let mut w = Writer::new();
            w.bytes(payload);
            state.next_teleport = state.next_teleport.wrapping_add(1);
            w.varint(state.next_teleport);
            packets.push((down(State::Play, play::PLAYER_POSITION)?, w.data));
            packets
        }
        cb::HELD_ITEM_CHANGE => {
            state.inventory.held = i16::from(Reader::new(payload).i8()?);
            vec![(down(State::Play, play::SET_HELD_SLOT)?, payload.to_vec())]
        }
        // 1.8's reasons 0 to 10 are 1.20.2's game events by the same numbers.
        cb::CHANGE_GAME_STATE => vec![(down(State::Play, play::GAME_EVENT)?, payload.to_vec())],
        cb::PLAYER_ABILITIES => {
            vec![(down(State::Play, play::PLAYER_ABILITIES)?, payload.to_vec())]
        }
        cb::DISCONNECT => vec![(down(State::Play, play::DISCONNECT)?, payload.to_vec())],
        cb::SERVER_DIFFICULTY => {
            let mut w = Writer::new();
            w.u8(Reader::new(payload).u8()?);
            w.bool(false);
            vec![(down(State::Play, play::CHANGE_DIFFICULTY)?, w.data)]
        }
        cb::PLUGIN_MESSAGE => {
            let mut r = Reader::new(payload);
            if r.string(20)? != "MC|Brand" {
                return Ok(Vec::new());
            }
            let mut w = Writer::new();
            w.string("minecraft:brand");
            w.bytes(r.rest());
            vec![(down(State::Play, play::CUSTOM_PAYLOAD)?, w.data)]
        }
        _ => Vec::new(),
    });
    Ok(out)
}

/// 1.8's login success names the profile's UUID as text and has no properties.
fn login_success(payload: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(payload);
    let text = r.string(36)?;
    let name = r.string(16)?;
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    let uuid = (hex.len() == 32)
        .then(|| u128::from_str_radix(&hex, 16).ok())
        .flatten()
        .ok_or_else(|| Error::new("invalid UUID in a 1.8 login success"))?;
    let mut w = Writer::new();
    w.uuid(uuid);
    w.string(&name);
    w.varint(0);
    Ok(w.data)
}

/// 1.8 packs a block position as x (26 bits), y (12), z (26); 1.14 and later as x, z, y.
fn position(packed: i64) -> i64 {
    let x = packed >> 38;
    let y = (packed << 26) >> 52;
    let z = (packed << 38) >> 38;
    ((x & 0x3FF_FFFF) << 38) | ((z & 0x3FF_FFFF) << 12) | (y & 0xFFF)
}

fn dimension_name(dimension: i8) -> &'static str {
    match dimension {
        -1 => "minecraft:the_nether",
        1 => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
}

/// The join: the configuration a 1.20.2 server would have sent first (its registries, then the end of
/// configuration), the play login, and the difficulty 1.8 sends apart.
fn join_game(payload: &[u8]) -> Result<(i8, Vec<(i32, Vec<u8>)>)> {
    use ids::configuration::clientbound as config;
    use ids::play::clientbound as play;
    let mut r = Reader::new(payload);
    let entity = r.i32()?;
    let mode = r.u8()?;
    let dimension = r.i8()?;
    let difficulty = r.u8()?;
    let max_players = r.u8()?;
    let level_type = r.string(16)?;
    let reduced_debug = r.bool()?;

    let mut registries = Writer::new();
    registries.nbt(&registry_codec());

    let mut login = Writer::new();
    login.i32(entity);
    login.bool(mode & 0x8 != 0);
    login.varint(3);
    for d in [0, -1, 1] {
        login.string(dimension_name(d));
    }
    login.varint(i32::from(max_players));
    login.varint(VIEW_DISTANCE);
    login.varint(VIEW_DISTANCE);
    login.bool(reduced_debug);
    login.bool(true); // respawn screen
    login.bool(false); // limited crafting
    login.string(dimension_name(dimension)); // dimension type
    login.string(dimension_name(dimension)); // level
    login.i64(0); // hashed seed
    login.u8(mode & 0x7);
    login.i8(-1); // no previous game mode
    login.bool(false); // debug world
    login.bool(level_type.eq_ignore_ascii_case("flat"));
    login.bool(false); // no death location
    login.varint(0); // portal cooldown

    let mut difficulty_body = Writer::new();
    difficulty_body.u8(difficulty);
    difficulty_body.bool(false);

    let packets = vec![
        (
            down(State::Configuration, config::REGISTRY_DATA)?,
            registries.data,
        ),
        (
            down(State::Configuration, config::FINISH_CONFIGURATION)?,
            Vec::new(),
        ),
        (down(State::Play, play::LOGIN)?, login.data),
        (
            down(State::Play, play::CHANGE_DIFFICULTY)?,
            difficulty_body.data,
        ),
    ];
    Ok((dimension, packets))
}

/// 1.8's respawn (dimension, difficulty, game mode, level type) as 1.20.2's, keeping nothing.
fn respawn(payload: &[u8]) -> Result<(i8, Vec<(i32, Vec<u8>)>)> {
    let mut r = Reader::new(payload);
    let dimension = r.i32()? as i8;
    let _difficulty = r.u8()?;
    let mode = r.u8()?;
    let level_type = r.string(16)?;
    let mut w = Writer::new();
    w.string(dimension_name(dimension)); // dimension type
    w.string(dimension_name(dimension)); // level
    w.i64(0); // hashed seed
    w.u8(mode & 0x7);
    w.i8(-1); // no previous game mode
    w.bool(false); // debug world
    w.bool(level_type.eq_ignore_ascii_case("flat"));
    w.bool(false); // no death location
    w.varint(0); // portal cooldown
    w.i8(0); // data kept
    Ok((
        dimension,
        vec![(down(State::Play, ids::play::clientbound::RESPAWN)?, w.data)],
    ))
}

/// 1.8 servers don't say how far they send chunks. Horizon keeps the chunks within this distance of the
/// centre: wide enough for a server's usual view distance of up to 12.
const VIEW_DISTANCE: i32 = 12;

/// The modern biome a 1.8 biome id became (1.13's and 1.18's renames), for chunks and the biome registry.
pub(super) fn biome(id: u8) -> &'static str {
    match id {
        0 => "minecraft:ocean",
        2 | 17 | 130 => "minecraft:desert",
        3 | 20 => "minecraft:windswept_hills",
        4 | 18 => "minecraft:forest",
        5 | 19 | 133 => "minecraft:taiga",
        6 | 134 => "minecraft:swamp",
        7 => "minecraft:river",
        8 => "minecraft:nether_wastes",
        9 => "minecraft:the_end",
        10 => "minecraft:frozen_ocean",
        11 => "minecraft:frozen_river",
        12 | 13 => "minecraft:snowy_plains",
        14 | 15 => "minecraft:mushroom_fields",
        16 => "minecraft:beach",
        21 | 22 | 149 => "minecraft:jungle",
        23 | 151 => "minecraft:sparse_jungle",
        24 => "minecraft:deep_ocean",
        25 => "minecraft:stony_shore",
        26 => "minecraft:snowy_beach",
        27 | 28 => "minecraft:birch_forest",
        29 | 157 => "minecraft:dark_forest",
        30 | 31 | 158 => "minecraft:snowy_taiga",
        32 | 33 => "minecraft:old_growth_pine_taiga",
        34 => "minecraft:windswept_forest",
        35 => "minecraft:savanna",
        36 => "minecraft:savanna_plateau",
        37 | 39 | 167 => "minecraft:badlands",
        38 | 166 => "minecraft:wooded_badlands",
        129 => "minecraft:sunflower_plains",
        131 | 162 => "minecraft:windswept_gravelly_hills",
        132 => "minecraft:flower_forest",
        140 => "minecraft:ice_spikes",
        155 | 156 => "minecraft:old_growth_birch_forest",
        160 | 161 => "minecraft:old_growth_spruce_taiga",
        163 | 164 => "minecraft:windswept_savanna",
        165 => "minecraft:eroded_badlands",
        _ => "minecraft:plains",
    }
}

const DIMENSION_TYPES: &[&str] = &[
    "minecraft:overworld",
    "minecraft:overworld_caves",
    "minecraft:the_end",
    "minecraft:the_nether",
];

const CHAT_TYPES: &[&str] = &[
    "minecraft:chat",
    "minecraft:emote_command",
    "minecraft:msg_command_incoming",
    "minecraft:msg_command_outgoing",
    "minecraft:say_command",
    "minecraft:team_msg_command_incoming",
    "minecraft:team_msg_command_outgoing",
];

/// 1.20.2's damage types. 1.8 sends none, but Horizon resolves damage by these names.
const DAMAGE_TYPES: &[&str] = &[
    "minecraft:arrow",
    "minecraft:bad_respawn_point",
    "minecraft:cactus",
    "minecraft:cramming",
    "minecraft:dragon_breath",
    "minecraft:drown",
    "minecraft:dry_out",
    "minecraft:explosion",
    "minecraft:fall",
    "minecraft:falling_anvil",
    "minecraft:falling_block",
    "minecraft:falling_stalactite",
    "minecraft:fireball",
    "minecraft:fireworks",
    "minecraft:fly_into_wall",
    "minecraft:freeze",
    "minecraft:generic",
    "minecraft:generic_kill",
    "minecraft:hot_floor",
    "minecraft:in_fire",
    "minecraft:in_wall",
    "minecraft:indirect_magic",
    "minecraft:lava",
    "minecraft:lightning_bolt",
    "minecraft:magic",
    "minecraft:mob_attack",
    "minecraft:mob_attack_no_aggro",
    "minecraft:mob_projectile",
    "minecraft:on_fire",
    "minecraft:out_of_world",
    "minecraft:outside_border",
    "minecraft:player_attack",
    "minecraft:player_explosion",
    "minecraft:sonic_boom",
    "minecraft:stalagmite",
    "minecraft:starve",
    "minecraft:sting",
    "minecraft:sweet_berry_bush",
    "minecraft:thorns",
    "minecraft:thrown",
    "minecraft:trident",
    "minecraft:unattributed_fireball",
    "minecraft:wither",
    "minecraft:wither_skull",
];

/// The biomes 1.8's ids become, each once, in the order [`biome`] first reaches them.
pub(super) fn biomes() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for id in 0..=u8::MAX {
        let name = biome(id);
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// The registry codec a 1.20.2 server sends in configuration, with names only: Horizon fills each
/// entry from its own copy of vanilla's data, as for a known pack, so no game data is written here.
fn registry_codec() -> Tag {
    fn registry<'a>(key: &'a str, names: &[&str]) -> (&'a str, Tag) {
        let entries = names
            .iter()
            .enumerate()
            .map(|(id, name)| {
                Tag::compound([
                    ("name", Tag::String((*name).into())),
                    ("id", Tag::Int(id as i32)),
                ])
            })
            .collect();
        (
            key,
            Tag::compound([
                ("type", Tag::String(key.into())),
                ("value", Tag::List(entries)),
            ]),
        )
    }
    Tag::compound([
        registry("minecraft:dimension_type", DIMENSION_TYPES),
        registry("minecraft:worldgen/biome", &biomes()),
        registry("minecraft:chat_type", CHAT_TYPES),
        registry("minecraft:damage_type", DAMAGE_TYPES),
    ])
}

/// A packet Horizon sends, brought down to 1.20.2 (`id` in 1.20.2's numbering for `writing`), as the
/// 1.8 packet bodies that stand for it. Empty sends nothing.
pub(super) fn serverbound(
    legacy: &Shared,
    writing: State,
    id: i32,
    payload: &[u8],
) -> Result<Vec<Vec<u8>>> {
    use ids::play::serverbound as play;
    let mut state = legacy.lock().unwrap();
    match writing {
        State::Handshake | State::Status => return Ok(vec![body(id, payload)]),
        State::Login => {
            let pinned = ServerVersion::V1_20_2.serverbound_id_up(State::Login, id);
            return Ok(match pinned {
                Some(ids::login::serverbound::HELLO) => {
                    let mut w = Writer::new();
                    w.string(&Reader::new(payload).string(16)?);
                    vec![body(id::login::LOGIN_START, &w.data)]
                }
                // Shared secret and token, VarInt-prefixed in both.
                Some(ids::login::serverbound::KEY) => {
                    vec![body(id::login::ENCRYPTION_RESPONSE, payload)]
                }
                _ => Vec::new(),
            });
        }
        State::Configuration => {
            if ServerVersion::V1_20_2.serverbound_id_up(State::Configuration, id)
                == Some(ids::configuration::serverbound::CLIENT_INFORMATION)
            {
                state.settings = Some(client_settings(payload)?);
            }
            return Ok(Vec::new());
        }
        State::Play => {}
    }
    use id::serverbound as sb;
    let pinned = ServerVersion::V1_20_2.serverbound_id_up(State::Play, id);
    let mut out = match id {
        sb_1_20_2::INTERACT => interact(payload)?,
        sb_1_20_2::SWING => {
            // The main hand only: 1.8 has no other.
            if Reader::new(payload).varint()? == 0 {
                vec![body(sb::ANIMATION, &[])]
            } else {
                Vec::new()
            }
        }
        sb_1_20_2::PLAYER_ACTION => player_action(&mut state, payload)?,
        sb_1_20_2::USE_ITEM_ON => use_item_on(&mut state, payload)?,
        sb_1_20_2::USE_ITEM => use_item(&mut state, payload)?,
        sb_1_20_2::PLAYER_COMMAND => player_command(payload)?,
        sb_1_20_2::CONTAINER_CLICK => {
            vec![body(
                sb::CLICK_WINDOW,
                &items::click(&mut state.inventory, payload)?,
            )]
        }
        sb_1_20_2::CONTAINER_CLOSE => vec![body(sb::CLOSE_WINDOW, &[Reader::new(payload).u8()?])],
        sb_1_20_2::PLAYER_ABILITIES => {
            let mut w = Writer::new();
            w.i8(Reader::new(payload).i8()?);
            w.f32(0.05); // flying speed
            w.f32(0.1); // walking speed
            vec![body(sb::PLAYER_ABILITIES, &w.data)]
        }
        _ => match pinned {
            Some(play::KEEP_ALIVE) => {
                let mut w = Writer::new();
                w.varint(Reader::new(payload).i64()? as i32);
                vec![body(sb::KEEP_ALIVE, &w.data)]
            }
            Some(play::CHAT) => chat(Reader::new(payload).string(256)?),
            Some(play::CHAT_COMMAND | play::CHAT_COMMAND_SIGNED) => {
                chat(format!("/{}", Reader::new(payload).string(32767)?))
            }
            // Each lays out its position, rotation and ground flag as 1.8 does.
            Some(play::MOVE_PLAYER_POS) => {
                follow(&mut state, payload)?;
                vec![body(sb::POSITION, payload)]
            }
            Some(play::MOVE_PLAYER_POS_ROT) => {
                follow(&mut state, payload)?;
                vec![body(sb::POSITION_AND_LOOK, payload)]
            }
            Some(play::MOVE_PLAYER_ROT) => vec![body(sb::LOOK, payload)],
            Some(play::MOVE_PLAYER_STATUS_ONLY) => vec![body(sb::PLAYER, payload)],
            Some(play::SET_CARRIED_ITEM) => {
                state.inventory.held = Reader::new(payload).i16()?.clamp(0, 8);
                vec![body(sb::HELD_ITEM_CHANGE, payload)]
            }
            // 0 respawn, 1 statistics in both.
            Some(play::CLIENT_COMMAND) => vec![body(sb::CLIENT_STATUS, payload)],
            Some(play::CLIENT_INFORMATION) => vec![client_settings(payload)?],
            Some(play::CUSTOM_PAYLOAD) => {
                let mut r = Reader::new(payload);
                if r.string(32767)? == "minecraft:brand" {
                    let mut w = Writer::new();
                    w.string("MC|Brand");
                    w.bytes(r.rest());
                    vec![body(sb::PLUGIN_MESSAGE, &w.data)]
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        },
    };
    if let Some(settings) = state.settings.take() {
        out.push(settings);
    }
    for confirmation in std::mem::take(&mut state.inventory.confirmations) {
        out.push(body(sb::CONFIRM_TRANSACTION, &confirmation));
    }
    Ok(out)
}

/// 1.20.2's interact (target, action, the point for an interaction at a point, the hand, sneaking) as
/// 1.8's use_entity, which has no hand: the off hand's are dropped.
fn interact(payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(payload);
    let target = r.varint()?;
    let action = r.varint()?;
    let mut w = Writer::new();
    w.varint(target);
    w.varint(action);
    if action == 2 {
        for _ in 0..3 {
            w.f32(r.f32()?);
        }
    }
    if action != 1 && r.varint()? != 0 {
        return Ok(Vec::new());
    }
    Ok(vec![body(id::serverbound::USE_ENTITY, &w.data)])
}

/// A block position as 1.20.2 packs it (x, z, y), packed as 1.8 packs it (x, y, z).
fn position_down(packed: i64) -> i64 {
    let [x, y, z] = crate::buf::unpack_block_pos(packed);
    ((i64::from(x) & 0x3FF_FFFF) << 38)
        | ((i64::from(y) & 0xFFF) << 26)
        | (i64::from(z) & 0x3FF_FFFF)
}

/// 1.20.2's player_action (status, position, face, sequence) as 1.8's player_digging.
fn player_action(state: &mut Legacy, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(payload);
    let status = r.varint()?;
    let pos = r.i64()?;
    let face = r.u8()?;
    let sequence = r.varint()?;
    if sequence > 0 {
        state.acks.push(sequence);
    }
    // 6 swaps the hands: 1.8 has one.
    if !(0..=5).contains(&status) {
        return Ok(Vec::new());
    }
    let mut w = Writer::new();
    w.u8(status as u8);
    w.i64(position_down(pos));
    w.u8(face);
    Ok(vec![body(id::serverbound::PLAYER_DIGGING, &w.data)])
}

/// 1.8's swords: right-clicking one blocks until the item is released.
fn is_sword(stack: Option<&items::Stack>) -> bool {
    stack.is_some_and(|s| matches!(s.id, 267 | 268 | 272 | 276 | 283))
}

fn placement(state: &Legacy, pos: i64, face: u8, cursor: [f32; 3]) -> Vec<u8> {
    let mut w = Writer::new();
    w.i64(pos);
    w.u8(face);
    items::write_stack(&mut w, state.inventory.in_hand());
    for c in cursor {
        w.u8((c * 16.0).clamp(0.0, 15.0) as u8);
    }
    body(id::serverbound::BLOCK_PLACEMENT, &w.data)
}

/// 1.20.2's use_item_on (hand, position, face, cursor, inside, sequence) as 1.8's block_placement.
fn use_item_on(state: &mut Legacy, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(payload);
    let hand = r.varint()?;
    let pos = r.i64()?;
    let face = r.varint()?;
    let cursor = [r.f32()?, r.f32()?, r.f32()?];
    let _inside = r.bool()?;
    let sequence = r.varint()?;
    if sequence > 0 {
        state.acks.push(sequence);
    }
    if hand != 0 {
        return Ok(Vec::new());
    }
    Ok(vec![placement(
        state,
        position_down(pos),
        face as u8,
        cursor,
    )])
}

/// 1.20.2's use_item (hand, sequence) as 1.8's block_placement at no block (position -1, face 255).
/// A sword would block until released, and Horizon never releases an item it can't use: the release goes
/// right after, rather than leave the player blocking on the server.
fn use_item(state: &mut Legacy, payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(payload);
    let hand = r.varint()?;
    let sequence = r.varint()?;
    if sequence > 0 {
        state.acks.push(sequence);
    }
    if hand != 0 {
        return Ok(Vec::new());
    }
    let mut out = vec![placement(state, -1, 255, [0.0; 3])];
    if is_sword(state.inventory.in_hand()) {
        let mut w = Writer::new();
        w.u8(5); // release the item in use
        w.i64(0);
        w.u8(255);
        out.push(body(id::serverbound::PLAYER_DIGGING, &w.data));
    }
    Ok(out)
}

/// 1.20.2's player_command as 1.8's entity_action: crouching, sprinting, leaving a bed and a horse's jump
/// keep their numbers; opening the inventory moves from 7 to 6; the rest have no 1.8 action.
fn player_command(payload: &[u8]) -> Result<Vec<Vec<u8>>> {
    let mut r = Reader::new(payload);
    let entity = r.varint()?;
    let action = match r.varint()? {
        a @ 0..=5 => a,
        7 => 6,
        _ => return Ok(Vec::new()),
    };
    let jump = r.varint()?;
    let mut w = Writer::new();
    w.varint(entity);
    w.varint(action);
    w.varint(jump);
    Ok(vec![body(id::serverbound::ENTITY_ACTION, &w.data)])
}

/// Where Horizon moved the player (a move's x, y, z), for the chunk centre the next packet read sends.
fn follow(state: &mut Legacy, payload: &[u8]) -> Result<()> {
    let mut r = Reader::new(payload);
    let (x, _, z) = (r.f64()?, r.f64()?, r.f64()?);
    state.position = Some([x, z]);
    Ok(())
}

fn chat(message: String) -> Vec<Vec<u8>> {
    let message: String = message.chars().take(MAX_CHAT).collect();
    let mut w = Writer::new();
    w.string(&message);
    vec![body(id::serverbound::CHAT, &w.data)]
}

/// 1.20.2's client information as 1.8's client settings: no main hand or listing flags, chat mode a byte.
fn client_settings(payload: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(payload);
    let locale = r.string(16)?;
    let view_distance = r.i8()?;
    let chat_mode = r.varint()?;
    let colors = r.bool()?;
    let skin = r.u8()?;
    let mut w = Writer::new();
    w.string(&locale);
    w.i8(view_distance);
    w.i8(chat_mode as i8);
    w.bool(colors);
    w.u8(skin);
    Ok(body(id::serverbound::CLIENT_SETTINGS, &w.data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{adapter::Session, buf::Codec};

    fn payload(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
        let mut w = Writer::new();
        f(&mut w);
        w.data
    }

    /// Feeds a 1.8 server's packets through a session, returning every pinned packet Horizon reads.
    fn read(session: &Session, id: i32, body: Vec<u8>) -> Vec<(i32, Vec<u8>)> {
        let mut out: Vec<_> = session.clientbound(id, body).unwrap().into_iter().collect();
        while let Some(next) = session.next_incoming() {
            out.push(next);
        }
        out
    }

    fn send(session: &Session, id: i32, payload: &[u8]) -> Vec<Vec<u8>> {
        let mut out: Vec<_> = session
            .serverbound(&body(id, payload))
            .unwrap()
            .into_iter()
            .collect();
        out.extend(session.take_outgoing());
        out
    }

    fn handshake(session: &Session) -> Vec<u8> {
        let hello = payload(|w| {
            w.varint(crate::PROTOCOL_VERSION);
            w.string("localhost");
            w.u16(25565);
            w.varint(2);
        });
        send(session, ids::handshake::serverbound::INTENTION, &hello).remove(0)
    }

    fn logged_in() -> Session {
        let session = Session::new(ServerVersion::V1_8);
        handshake(&session);
        let success = payload(|w| {
            w.string("069a79f4-44e9-4726-a5be-fca90e38aaf5");
            w.string("Notch");
        });
        read(&session, id::login::LOGIN_SUCCESS, success);
        send(&session, ids::login::serverbound::LOGIN_ACKNOWLEDGED, &[]);
        session
    }

    fn join() -> Vec<u8> {
        payload(|w| {
            w.i32(1234);
            w.u8(0x8 | 1); // hardcore creative
            w.i8(0); // overworld
            w.u8(2);
            w.u8(60);
            w.string("default");
            w.bool(false);
        })
    }

    /// Logged in, joined, and Horizon done configuring.
    fn in_play() -> Session {
        let session = logged_in();
        read(&session, id::clientbound::JOIN_GAME, join());
        send(
            &session,
            ids::configuration::serverbound::FINISH_CONFIGURATION,
            &[],
        );
        session
    }

    #[test]
    fn the_handshake_states_protocol_47() {
        let session = Session::new(ServerVersion::V1_8);
        let sent = handshake(&session);
        let mut r = Reader::new(&sent);
        assert_eq!(r.varint().unwrap(), 0);
        assert_eq!(r.varint().unwrap(), 47);
        assert_eq!(r.string(255).unwrap(), "localhost");
        assert_eq!(session.writing(), State::Login);
    }

    #[test]
    fn login_start_names_the_player_only() {
        let session = Session::new(ServerVersion::V1_8);
        handshake(&session);
        let hello = payload(|w| {
            w.string("Steve");
            w.uuid(7);
        });
        let sent = send(&session, ids::login::serverbound::HELLO, &hello);
        assert_eq!(sent, vec![body(0, &payload(|w| w.string("Steve")))]);
    }

    #[test]
    fn login_success_becomes_a_profile_and_configuration_stays_quiet() {
        let session = Session::new(ServerVersion::V1_8);
        handshake(&session);
        let success = payload(|w| {
            w.string("069a79f4-44e9-4726-a5be-fca90e38aaf5");
            w.string("Notch");
        });
        let read = read(&session, id::login::LOGIN_SUCCESS, success);
        assert_eq!(read.len(), 1);
        let (id, body) = &read[0];
        assert_eq!(*id, ids::login::clientbound::LOGIN_FINISHED);
        let mut r = Reader::new(body);
        assert_eq!(r.uuid().unwrap(), 0x069a79f444e94726a5befca90e38aaf5);
        assert_eq!(r.string(16).unwrap(), "Notch");
        assert_eq!(session.reading(), State::Configuration);
        // Horizon's acknowledgement and configuration replies have no 1.8 packet.
        assert!(send(&session, ids::login::serverbound::LOGIN_ACKNOWLEDGED, &[]).is_empty());
        let brand = payload(|w| {
            w.string("minecraft:brand");
            w.string("horizon");
        });
        assert!(
            send(
                &session,
                ids::configuration::serverbound::CUSTOM_PAYLOAD,
                &brand
            )
            .is_empty()
        );
    }

    #[test]
    fn join_game_configures_horizon_then_logs_it_into_play() {
        use crate::packets::{configuration, play};
        let session = logged_in();
        let read = read(&session, id::clientbound::JOIN_GAME, join());
        let ids: Vec<i32> = read.iter().map(|(id, _)| *id).collect();
        let registries = read
            .iter()
            .filter(|(id, _)| *id == ids::configuration::clientbound::REGISTRY_DATA)
            .map(|(_, body)| configuration::RegistryData::read(&mut Reader::new(body)).unwrap())
            .collect::<Vec<_>>();
        let biomes = registries
            .iter()
            .find(|r| r.registry == "minecraft:worldgen/biome")
            .expect("biomes are synced");
        assert!(biomes.entries.iter().any(|e| e.id == "minecraft:plains"));
        assert!(biomes.entries.iter().all(|e| e.data.is_none()));
        let finish = ids
            .iter()
            .position(|&id| id == ids::configuration::clientbound::FINISH_CONFIGURATION)
            .expect("configuration finishes");
        let login = ids
            .iter()
            .position(|&id| id == ids::play::clientbound::LOGIN)
            .expect("the play login follows");
        assert!(finish < login);
        assert_eq!(session.reading(), State::Play);
        let play::Clientbound::Login(login) =
            play::Clientbound::decode(read[login].0, &read[login].1).unwrap()
        else {
            panic!("not a login");
        };
        assert_eq!(login.player_id, 1234);
        assert!(login.hardcore);
    }

    #[test]
    fn keep_alives_go_both_ways() {
        let session = in_play();
        let read = read(
            &session,
            id::clientbound::KEEP_ALIVE,
            payload(|w| w.varint(99)),
        );
        assert_eq!(
            read,
            vec![(ids::play::clientbound::KEEP_ALIVE, payload(|w| w.i64(99)))]
        );
        let sent = send(
            &session,
            ids::play::serverbound::KEEP_ALIVE,
            &payload(|w| w.i64(99)),
        );
        assert_eq!(sent, vec![body(0x00, &payload(|w| w.varint(99)))]);
    }

    #[test]
    fn a_teleport_gets_an_id_and_its_confirmation_is_not_sent() {
        let session = in_play();
        let teleport = payload(|w| {
            w.f64(1.5);
            w.f64(64.0);
            w.f64(-2.5);
            w.f32(90.0);
            w.f32(0.0);
            w.i8(0);
        });
        let read = read(&session, id::clientbound::POSITION_AND_LOOK, teleport);
        let ids: Vec<i32> = read.iter().map(|(id, _)| *id).collect();
        // The first position also tells Horizon which chunk to keep chunks around.
        assert_eq!(
            ids,
            [
                ids::play::clientbound::SET_CHUNK_CACHE_CENTER,
                ids::play::clientbound::PLAYER_POSITION
            ]
        );
        let confirm = payload(|w| w.varint(1));
        assert!(
            send(
                &session,
                ids::play::serverbound::ACCEPT_TELEPORTATION,
                &confirm
            )
            .is_empty()
        );
    }

    #[test]
    fn chunks_arrive_around_the_player() {
        use crate::packets::play;
        let session = in_play();
        let teleport = payload(|w| {
            w.f64(40.0);
            w.f64(64.0);
            w.f64(-20.0);
            w.f32(0.0);
            w.f32(0.0);
            w.i8(0);
        });
        let read_center = read(&session, id::clientbound::POSITION_AND_LOOK, teleport);
        let play::Clientbound::SetChunkCacheCenter(center) =
            play::Clientbound::decode(read_center[0].0, &read_center[0].1).unwrap()
        else {
            panic!("no chunk centre");
        };
        assert_eq!((center.x, center.z), (2, -2));
        let column = chunks::tests::stone_column(2, -2);
        let out = read(&session, id::clientbound::CHUNK_DATA, column);
        assert_eq!(out.len(), 1);
        let play::Clientbound::LevelChunkWithLight(chunk) =
            play::Clientbound::decode(out[0].0, &out[0].1).unwrap()
        else {
            panic!("not a chunk");
        };
        assert_eq!((chunk.x, chunk.z), (2, -2));
        let block_bits = ServerVersion::V1_20_2.ids().unwrap().state_bits().1;
        let biome_bits = crate::registry::ceil_log2(biomes().len());
        let sections = chunk.chunk.sections(24, block_bits, biome_bits).unwrap();
        // The overworld's four sections below y = 0 are empty; 1.8's section 0 is the fifth.
        assert_eq!(sections[3].non_empty, 0);
        assert_eq!(sections[4].non_empty, 4096);
        assert_eq!(sections[5].non_empty, 0);
        // Moving to the next chunk moves the centre with the next packet read.
        let moved = payload(|w| {
            w.f64(56.5);
            w.f64(64.0);
            w.f64(-20.0);
            w.u8(1); // on the ground
        });
        send(&session, ids::play::serverbound::MOVE_PLAYER_POS, &moved);
        let after = read(
            &session,
            id::clientbound::KEEP_ALIVE,
            payload(|w| w.varint(5)),
        );
        assert_eq!(after[0].0, ids::play::clientbound::SET_CHUNK_CACHE_CENTER);
    }

    #[test]
    fn attacks_and_swings_reach_the_server_as_1_8_packets() {
        let session = in_play();
        // 26.3's attack and punch: the target's id, and nothing.
        let attack = send(
            &session,
            ids::play::serverbound::ATTACK,
            &payload(|w| w.varint(42)),
        );
        assert_eq!(
            attack,
            vec![body(
                0x02,
                &payload(|w| {
                    w.varint(42);
                    w.varint(1);
                })
            )]
        );
        let swing = send(&session, ids::play::serverbound::PUNCH, &[]);
        assert_eq!(swing, vec![body(0x0A, &[])]);
    }

    #[test]
    fn a_1_8_player_spawns_as_a_player() {
        use crate::packets::play;
        let session = in_play();
        let spawn = payload(|w| {
            w.varint(7);
            w.uuid(0xABCD);
            w.i32(32 * 10);
            w.i32(32 * 64);
            w.i32(0);
            w.i8(0);
            w.i8(0);
            w.i16(0);
            w.u8(0x7F);
        });
        let out = read(&session, id::clientbound::SPAWN_PLAYER, spawn);
        let play::Clientbound::AddEntity(e) =
            play::Clientbound::decode(out[0].0, &out[0].1).unwrap()
        else {
            panic!("not an entity");
        };
        assert_eq!(
            (e.id, e.uuid, e.kind),
            (7, 0xABCD, crate::adapter::pinned_ids::entity_type::PLAYER)
        );
        assert_eq!(e.position, [10.0, 64.0, 0.0]);
    }

    /// Every packet this step makes for a 1.8 one comes out of the shared steps as a pinned packet Horizon
    /// can decode: the 1.20.2 layouts written here are the ones the steps read.
    #[test]
    fn translated_packets_decode_as_pinned_packets() {
        use crate::packets::play;
        let session = in_play();
        let cases: Vec<(i32, Vec<u8>)> = vec![
            (
                id::clientbound::SCOREBOARD_OBJECTIVE,
                payload(|w| {
                    w.string("kills");
                    w.i8(0);
                    w.string("Kills");
                    w.string("integer");
                }),
            ),
            (
                id::clientbound::DISPLAY_SCOREBOARD,
                payload(|w| {
                    w.i8(1);
                    w.string("kills");
                }),
            ),
            (
                id::clientbound::UPDATE_SCORE,
                payload(|w| {
                    w.string("Steve");
                    w.i8(0);
                    w.string("kills");
                    w.varint(3);
                }),
            ),
            (
                id::clientbound::TEAMS,
                payload(|w| {
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
                }),
            ),
            (
                id::clientbound::TITLE,
                payload(|w| {
                    w.varint(0);
                    w.string(r#"{"text":"Fight!"}"#);
                }),
            ),
            (
                id::clientbound::TITLE,
                payload(|w| {
                    w.varint(2);
                    w.i32(10);
                    w.i32(70);
                    w.i32(20);
                }),
            ),
            (
                id::clientbound::PLAYER_LIST_HEADER_FOOTER,
                payload(|w| {
                    w.string(r#"{"text":"PvP"}"#);
                    w.string(r#"{"text":"play.example"}"#);
                }),
            ),
            (
                id::clientbound::SOUND_EFFECT,
                payload(|w| {
                    w.string("game.player.hurt");
                    w.i32(80);
                    w.i32(512);
                    w.i32(0);
                    w.f32(1.0);
                    w.u8(63);
                }),
            ),
            (
                id::clientbound::EXPLOSION,
                payload(|w| {
                    w.f32(1.0);
                    w.f32(64.0);
                    w.f32(1.0);
                    w.f32(4.0);
                    w.i32(1);
                    w.bytes(&[1, 0, 255]);
                    w.f32(0.0);
                    w.f32(0.5);
                    w.f32(0.0);
                }),
            ),
            (
                id::clientbound::ENTITY_EFFECT,
                payload(|w| {
                    w.varint(1234);
                    w.i8(1); // speed
                    w.i8(1);
                    w.varint(600);
                    w.bool(false);
                }),
            ),
            (
                id::clientbound::ENTITY_TELEPORT,
                payload(|w| {
                    w.varint(5);
                    w.i32(32);
                    w.i32(64 * 32);
                    w.i32(0);
                    w.i8(0);
                    w.i8(0);
                    w.bool(true);
                }),
            ),
            (
                id::clientbound::PLAYER_LIST_ITEM,
                payload(|w| {
                    w.varint(0);
                    w.varint(1);
                    w.uuid(42);
                    w.string("Notch");
                    w.varint(0);
                    w.varint(0);
                    w.varint(35);
                    w.bool(true);
                    w.string(r#"{"text":"[VIP] Notch"}"#);
                }),
            ),
            (
                id::clientbound::WINDOW_ITEMS,
                payload(|w| {
                    w.u8(0);
                    w.i16(2);
                    w.i16(-1);
                    w.i16(-1);
                }),
            ),
            (
                id::clientbound::OPEN_WINDOW,
                payload(|w| {
                    w.u8(1);
                    w.string("minecraft:chest");
                    w.string(r#"{"text":"Chest"}"#);
                    w.u8(27);
                }),
            ),
        ];
        for (id, body) in cases {
            let out = read(&session, id, body);
            assert!(!out.is_empty(), "1.8 packet {id:#x} dropped");
            for (pinned, body) in out {
                play::Clientbound::decode(pinned, &body)
                    .unwrap_or_else(|e| panic!("1.8 packet {id:#x} as pinned {pinned}: {e}"));
            }
        }
    }

    #[test]
    fn digging_reaches_1_8_and_is_acknowledged() {
        let session = in_play();
        let dig = payload(|w| {
            w.varint(0); // start digging
            w.i64(crate::buf::pack_block_pos([10, 64, -3]));
            w.u8(1); // top face
            w.varint(9); // sequence
        });
        let sent = send(&session, ids::play::serverbound::PLAYER_ACTION, &dig);
        let packed_1_8 = ((10i64 & 0x3FF_FFFF) << 38) | (64 << 26) | (-3i64 & 0x3FF_FFFF);
        assert_eq!(
            sent,
            vec![body(
                0x07,
                &payload(|w| {
                    w.u8(0);
                    w.i64(packed_1_8);
                    w.u8(1);
                })
            )]
        );
        // Horizon hears its sequence acknowledged with the next packet read.
        let next = read(
            &session,
            id::clientbound::KEEP_ALIVE,
            payload(|w| w.varint(1)),
        );
        assert_eq!(
            next[0],
            (
                ids::play::clientbound::BLOCK_CHANGED_ACK,
                payload(|w| w.varint(9))
            )
        );
    }

    #[test]
    fn spawn_positions_are_repacked() {
        let packed_1_8 = ((-5i64 & 0x3FF_FFFF) << 38) | (70 << 26) | (300 & 0x3FF_FFFF);
        assert_eq!(
            crate::buf::unpack_block_pos(position(packed_1_8)),
            [-5, 70, 300]
        );
    }

    #[test]
    fn client_settings_wait_for_the_first_play_packet() {
        let session = logged_in();
        let info = payload(|w| {
            w.string("fr_fr");
            w.i8(12);
            w.varint(0);
            w.bool(true);
            w.u8(0x7F);
            w.varint(1);
            w.bool(false);
            w.bool(true);
            w.varint(0); // particles: all
        });
        assert!(
            send(
                &session,
                ids::configuration::serverbound::CLIENT_INFORMATION,
                &info
            )
            .is_empty()
        );
        read(&session, id::clientbound::JOIN_GAME, join());
        send(
            &session,
            ids::configuration::serverbound::FINISH_CONFIGURATION,
            &[],
        );
        let sent = send(
            &session,
            ids::play::serverbound::KEEP_ALIVE,
            &payload(|w| w.i64(1)),
        );
        let settings = payload(|w| {
            w.string("fr_fr");
            w.i8(12);
            w.i8(0);
            w.bool(true);
            w.u8(0x7F);
        });
        assert_eq!(sent.last(), Some(&body(0x15, &settings)));
    }

    #[test]
    fn every_1_8_biome_has_a_registry_entry() {
        let biomes = biomes();
        for id in 0..=u8::MAX {
            assert!(biomes.contains(&biome(id)));
        }
        assert_eq!(biomes[0], "minecraft:ocean");
    }
}
