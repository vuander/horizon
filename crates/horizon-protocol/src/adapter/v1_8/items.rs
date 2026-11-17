//! 1.8's item stacks, inventories and container windows as 1.20.2's, and Horizon's clicks back.
//!
//! A 1.8 stack is an item number, a count, a damage value (which also picked the variant: wool colour,
//! potion, ...) and NBT. Which 1.20.2 item it is comes from vanilla's data fixers, as for blocks: the stack
//! is upgraded as a 1.8 save's would be, up to 1.20.2's data version, which keeps the NBT in the form
//! 1.20.2 sent; the shared steps then make components of it.
//!
//! A 1.8 server refuses a click unless the packet names the stack that was in the slot, so the stacks it
//! sent are kept here, as it sent them.
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use super::down;
use crate::{
    Result, State,
    buf::{Reader, Writer},
    ids::play::clientbound as play,
};
use horizon_core::nbt::{self, NamedTag, Tag};

/// 1.20.2's DataVersion: stacks are upgraded this far, no further.
const DATA_VERSION_1_20_2: i32 = 3578;

/// A 1.8 stack as the server sent it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Stack {
    pub id: i16,
    pub count: i8,
    pub damage: i16,
    pub tag: Option<Tag>,
}

/// A 1.8 slot: an item number (-1 for none), then the count, damage and a named NBT compound (0 for none).
pub(super) fn read_stack(r: &mut Reader) -> Result<Option<Stack>> {
    let id = r.i16()?;
    if id < 0 {
        return Ok(None);
    }
    let count = r.i8()?;
    let damage = r.i16()?;
    let tag = if r.peek().first() == Some(&0) {
        r.u8()?;
        None
    } else {
        let (named, used) = nbt::read_named(r.peek(), nbt::Limits::network())
            .map_err(|e| crate::Error::new(format!("1.8 item NBT: {e}")))?;
        r.bytes(used)?;
        Some(named.tag)
    };
    Ok(Some(Stack {
        id,
        count,
        damage,
        tag,
    }))
}

pub(super) fn write_stack(w: &mut Writer, stack: Option<&Stack>) {
    let Some(stack) = stack else {
        w.i16(-1);
        return;
    };
    w.i16(stack.id);
    w.i8(stack.count);
    w.i16(stack.damage);
    let named = stack.tag.as_ref().and_then(|tag| {
        nbt::write_named(
            &NamedTag {
                name: "".into(),
                tag: tag.clone(),
            },
            nbt::Limits::network(),
        )
        .ok()
    });
    match named {
        Some(bytes) => w.bytes(&bytes),
        None => w.u8(0),
    }
}

/// 1.20.2's item number and NBT for a 1.8 stack; `None` when there is nothing to read it with (no prepared
/// data fixer tables) or the item has no 1.20.2 counterpart.
fn upgrade(stack: &Stack) -> Option<(i32, Option<Tag>)> {
    type Key = (i16, i16, Option<Vec<u8>>);
    static CACHE: OnceLock<Mutex<HashMap<Key, Option<(i32, Option<Tag>)>>>> = OnceLock::new();
    let key: Key = (
        stack.id,
        stack.damage,
        stack
            .tag
            .as_ref()
            .and_then(|t| nbt::write_network(t, nbt::Limits::network()).ok()),
    );
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return hit.clone();
    }
    let upgraded = std::panic::catch_unwind(|| upgrade_uncached(stack)).unwrap_or(None);
    let mut cache = cache.lock().unwrap();
    if cache.len() < 4096 {
        cache.insert(key, upgraded.clone());
    }
    upgraded
}

fn upgrade_uncached(stack: &Stack) -> Option<(i32, Option<Tag>)> {
    use horizon_world::datafix::{self, dynamic, schema::Ty};
    if !datafix::tables_available() {
        return None;
    }
    let mut entries = vec![
        ("id", Tag::Short(stack.id)),
        ("Count", Tag::Byte(1)),
        ("Damage", Tag::Short(stack.damage)),
    ];
    if let Some(tag) = &stack.tag {
        entries.push(("tag", tag.clone()));
    }
    let mut item = Tag::compound(entries);
    datafix::update(Ty::ItemStack, &mut item, -1, DATA_VERSION_1_20_2);
    let name = dynamic::as_str(item.get("id"))?;
    let id = super::super::v1_20_2_ids::OLD_ITEM_NAMES
        .iter()
        .position(|n| *n == name)?;
    Some((id as i32, item.get("tag").cloned()))
}

/// A 1.8 stack as a 1.20.2 slot: present, item number, count, NBT.
pub(super) fn write_item(w: &mut Writer, stack: Option<&Stack>) {
    let upgraded = stack.and_then(|s| upgrade(s).map(|(id, tag)| (id, s.count, tag)));
    match upgraded {
        Some((id, count, tag)) if id != 0 && count > 0 => {
            w.bool(true);
            w.varint(id);
            w.i8(count);
            w.nbt(&tag.unwrap_or(Tag::End));
        }
        _ => w.bool(false),
    }
}

/// 1.20.2's menu number for a 1.8 window type and size; `None` for windows 1.20.2 can't show this way.
fn menu(kind: &str, slots: u8) -> Option<i32> {
    // 1.20.2's menu registry: generic_9x1 to generic_9x6 (0 to 5), generic_3x3, anvil, beacon,
    // blast_furnace, brewing_stand, cartography_table, crafting, enchantment, furnace, grindstone, hopper,
    // lectern, loom, merchant, shulker_box, smithing, smoker, stonecutter.
    Some(match kind {
        "minecraft:chest" | "minecraft:container" => i32::from(slots / 9).clamp(1, 6) - 1,
        "minecraft:dispenser" | "minecraft:dropper" => 6,
        "minecraft:anvil" => 7,
        "minecraft:beacon" => 8,
        "minecraft:brewing_stand" => 10,
        "minecraft:crafting_table" => 12,
        "minecraft:enchanting_table" => 13,
        "minecraft:furnace" => 14,
        "minecraft:hopper" => 16,
        "minecraft:villager" => 19,
        _ => return None,
    })
}

/// What the inventory side of a connection remembers.
#[derive(Default)]
pub(super) struct Inventory {
    /// The stacks the server last put in each window's slots, as it sent them.
    slots: HashMap<(i8, i16), Stack>,
    /// The hotbar slot in hand (0 to 8).
    pub held: i16,
    /// 1.8 numbers each click; the server confirms or refuses it by that number.
    next_action: i16,
    /// Refused clicks to acknowledge, as 1.8 confirm_transaction bodies.
    pub confirmations: Vec<Vec<u8>>,
}

impl Inventory {
    fn set(&mut self, window: i8, slot: i16, stack: Option<Stack>) {
        match stack {
            Some(stack) => self.slots.insert((window, slot), stack),
            None => self.slots.remove(&(window, slot)),
        };
    }

    /// The stack in hand, as 1.8 last sent it (the player's window, hotbar slots 36 to 44).
    pub fn in_hand(&self) -> Option<&Stack> {
        self.slots.get(&(0, 36 + self.held))
    }
}

/// 1.8's set_slot, window_items, open_window, close_window, window_property and confirm_transaction.
pub(super) fn clientbound(
    inv: &mut Inventory,
    id: i32,
    payload: &[u8],
) -> Result<Vec<(i32, Vec<u8>)>> {
    use super::id::clientbound as cb;
    let mut r = Reader::new(payload);
    let mut w = Writer::new();
    Ok(match id {
        cb::SET_SLOT => {
            let window = r.i8()?;
            let slot = r.i16()?;
            let stack = read_stack(&mut r)?;
            w.i8(window);
            w.varint(0); // state id: 1.8 has none
            w.i16(slot);
            write_item(&mut w, stack.as_ref());
            inv.set(window, slot, stack);
            vec![(down(State::Play, play::CONTAINER_SET_SLOT)?, w.data)]
        }
        cb::WINDOW_ITEMS => {
            let window = r.u8()? as i8;
            let n = r.i16()?.max(0);
            w.u8(window as u8);
            w.varint(0);
            w.varint(i32::from(n));
            for slot in 0..n {
                let stack = read_stack(&mut r)?;
                write_item(&mut w, stack.as_ref());
                inv.set(window, slot, stack);
            }
            w.bool(false); // carried item
            vec![(down(State::Play, play::CONTAINER_SET_CONTENT)?, w.data)]
        }
        cb::OPEN_WINDOW => {
            let window = r.u8()?;
            let kind = r.string(32)?;
            let title = r.string(32767)?;
            let slots = r.u8()?;
            let Some(menu) = menu(&kind, slots) else {
                return Ok(Vec::new());
            };
            inv.slots.retain(|(w, _), _| *w == 0);
            w.varint(i32::from(window));
            w.varint(menu);
            w.string(&title);
            vec![(down(State::Play, play::OPEN_SCREEN)?, w.data)]
        }
        cb::CLOSE_WINDOW => {
            w.u8(r.u8()?);
            vec![(down(State::Play, play::CONTAINER_CLOSE)?, w.data)]
        }
        // Window, property and value: alike.
        cb::WINDOW_PROPERTY => vec![(
            down(State::Play, play::CONTAINER_SET_DATA)?,
            payload.to_vec(),
        )],
        cb::CONFIRM_TRANSACTION => {
            let window = r.i8()?;
            let action = r.i16()?;
            if !r.bool()? {
                // A refused click stays refused until the client says it saw that: say it for Horizon,
                // which then gets the window's contents again.
                let mut c = Writer::new();
                c.i8(window);
                c.i16(action);
                c.bool(true);
                inv.confirmations.push(c.data);
            }
            Vec::new()
        }
        _ => Vec::new(),
    })
}

/// Horizon's container_click (1.20.2: window, state id, slot, button, mode, ...) as 1.8's click_window,
/// naming the stack the server last put in that slot.
pub(super) fn click(inv: &mut Inventory, payload: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader::new(payload);
    let window = r.u8()?;
    let _state = r.varint()?;
    let slot = r.i16()?;
    let button = r.i8()?;
    let mode = r.varint()?;
    inv.next_action = inv.next_action.wrapping_add(1);
    let mut w = Writer::new();
    w.u8(window);
    w.i16(slot);
    w.i8(button);
    w.i16(inv.next_action);
    w.i8(mode as i8);
    write_stack(&mut w, inv.slots.get(&(window as i8, slot)));
    Ok(w.data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(tag: Tag) -> Vec<u8> {
        nbt::write_named(
            &NamedTag {
                name: "".into(),
                tag,
            },
            nbt::Limits::network(),
        )
        .unwrap()
    }

    #[test]
    fn stacks_read_with_and_without_nbt() {
        let mut w = Writer::new();
        w.i16(276); // diamond sword
        w.i8(1);
        w.i16(3);
        let ench = Tag::compound([(
            "ench",
            Tag::List(vec![Tag::compound([
                ("id", Tag::Short(16)),
                ("lvl", Tag::Short(5)),
            ])]),
        )]);
        w.bytes(&named(ench.clone()));
        w.i16(-1);
        w.i16(1);
        w.i8(64);
        w.i16(0);
        w.u8(0);
        let mut r = Reader::new(&w.data);
        let sword = read_stack(&mut r).unwrap().unwrap();
        assert_eq!((sword.id, sword.count, sword.damage), (276, 1, 3));
        assert_eq!(sword.tag, Some(ench));
        assert_eq!(read_stack(&mut r).unwrap(), None);
        let stone = read_stack(&mut r).unwrap().unwrap();
        assert_eq!((stone.id, stone.count, &stone.tag), (1, 64, &None));
        assert_eq!(r.remaining(), 0);
        // And back, byte for byte.
        let mut back = Writer::new();
        write_stack(&mut back, Some(&sword));
        write_stack(&mut back, None);
        write_stack(&mut back, Some(&stone));
        assert_eq!(back.data, w.data);
    }

    #[test]
    fn a_click_names_the_stack_the_server_put_there() {
        let mut inv = Inventory::default();
        let mut w = Writer::new();
        w.i8(0);
        w.i16(36);
        w.i16(267); // iron sword
        w.i8(1);
        w.i16(0);
        w.u8(0);
        clientbound(&mut inv, super::super::id::clientbound::SET_SLOT, &w.data).unwrap();
        let mut click_body = Writer::new();
        click_body.u8(0);
        click_body.varint(7);
        click_body.i16(36);
        click_body.i8(0);
        click_body.varint(0);
        click_body.varint(0);
        click_body.bool(false);
        let sent = click(&mut inv, &click_body.data).unwrap();
        let mut r = Reader::new(&sent);
        assert_eq!(
            (r.u8().unwrap(), r.i16().unwrap(), r.i8().unwrap()),
            (0, 36, 0)
        );
        assert_eq!(r.i16().unwrap(), 1); // first action number
        assert_eq!(r.i8().unwrap(), 0);
        assert_eq!(read_stack(&mut r).unwrap().map(|s| s.id), Some(267));
        assert_eq!(inv.in_hand().map(|s| s.id), Some(267));
    }

    #[test]
    fn a_refused_click_is_acknowledged() {
        let mut inv = Inventory::default();
        let mut w = Writer::new();
        w.i8(0);
        w.i16(4);
        w.bool(false);
        let out = clientbound(
            &mut inv,
            super::super::id::clientbound::CONFIRM_TRANSACTION,
            &w.data,
        )
        .unwrap();
        assert!(out.is_empty());
        assert_eq!(inv.confirmations, vec![vec![0, 0, 4, 1]]);
    }

    #[test]
    fn chests_open_as_generic_menus_by_size() {
        assert_eq!(menu("minecraft:chest", 27), Some(2));
        assert_eq!(menu("minecraft:chest", 54), Some(5));
        assert_eq!(menu("EntityHorse", 2), None);
    }

    #[test]
    fn vanilla_items_upgrade() {
        if !horizon_world::datafix::tables_available() {
            eprintln!("skipping: no prepared resources");
            return;
        }
        let wool = |damage| {
            upgrade(&Stack {
                id: 35,
                count: 1,
                damage,
                tag: None,
            })
        };
        assert_ne!(wool(0).map(|w| w.0), wool(14).map(|w| w.0));
    }
}
