//! Profiles: what keymaps and HDR curves share. A profile is either named and shared by the games that
//! choose it, or a game's own: unnamed, offered to that game alone, made from the profile in force for the
//! game and kept while the game uses another one, until it is deleted. Contents move between the two
//! kinds by copying: a named profile (or the none in force, id 0) can be loaded into a game's own, and a
//! game's own can be saved as a new named profile or over an existing one. What a profile holds is its
//! body (`B`): a keymap's layers, a curve's parameters.

use std::sync::Arc;

use crate::kit::replace;

/// A named profile or a game's own, holding a body `B`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Profile<B> {
    /// Its number, kept across renames. 0 is the none in force (Default for keymaps, Off for curves),
    /// which no profile has.
    pub id: u16,
    /// Empty for a game's own.
    pub name: String,
    /// The id of the game whose own profile this is. None for a named profile, which any game may choose.
    pub owner: Option<Arc<str>>,
    #[serde(flatten)]
    pub body: B,
}

impl<B: Default> Default for Profile<B> {
    fn default() -> Profile<B> {
        Profile::with(0, "", B::default())
    }
}

impl<B> Profile<B> {
    /// A named profile with `body`.
    pub fn with(id: u16, name: &str, body: B) -> Profile<B> {
        Profile { id, name: name.to_owned(), owner: None, body }
    }
}

/// Every profile of one kind, by id. Each change counts, and a profile's version is the count at its last
/// change, so that a profile can be compared with one sent earlier by its id and version alone.
#[derive(Debug, Clone)]
pub struct Profiles<B> {
    list: Vec<Profile<B>>,
    /// The changes so far.
    changes: u64,
    /// The profiles changed since they were read, with the count at their last change.
    versions: Vec<(u16, u64)>,
}

impl<B> Default for Profiles<B> {
    fn default() -> Profiles<B> {
        Profiles { list: Vec::new(), changes: 0, versions: Vec::new() }
    }
}

/// Equal when the profiles are; their versions don't count.
impl<B: PartialEq> PartialEq for Profiles<B> {
    fn eq(&self, other: &Profiles<B>) -> bool {
        self.list == other.list
    }
}

impl<B: Eq> Eq for Profiles<B> {}

impl<B: Clone + Default + PartialEq> Profiles<B> {
    /// These profiles, as saved.
    pub fn from_list(list: Vec<Profile<B>>) -> Profiles<B> {
        Profiles { list, ..Profiles::default() }
    }

    pub fn list(&self) -> &[Profile<B>] {
        &self.list
    }

    pub fn get(&self, id: u16) -> Option<&Profile<B>> {
        self.list.iter().find(|p| p.id == id)
    }

    /// Profile `id` to change. It counts as changed whether or not it is.
    pub fn get_mut(&mut self, id: u16) -> Option<&mut Profile<B>> {
        self.changed(id);
        self.list.iter_mut().find(|p| p.id == id)
    }

    /// Changes profile `id` with `f`, which returns whether it changed anything. Returns that, and false if
    /// the profile isn't there.
    pub fn edit(&mut self, id: u16, f: impl FnOnce(&mut Profile<B>) -> bool) -> bool {
        let changed = self.list.iter_mut().find(|p| p.id == id).is_some_and(f);
        if changed {
            self.changed(id);
        }
        changed
    }

    /// Puts `profile` in place of the profile with its id, if there is one.
    pub fn replace(&mut self, profile: Profile<B>) {
        self.edit(profile.id, |p| replace(p, profile));
    }

    /// Profile `id`'s version: the count of changes at its last change, or 0 if it hasn't changed since the
    /// profiles were read.
    pub fn version(&self, id: u16) -> u64 {
        self.versions.iter().find(|(p, _)| *p == id).map_or(0, |(_, v)| *v)
    }

    /// Counts a change to profile `id`.
    fn changed(&mut self, id: u16) {
        self.changes += 1;
        let version = self.changes;
        match self.versions.iter_mut().find(|(p, _)| *p == id) {
            Some((_, v)) => *v = version,
            None => self.versions.push((id, version)),
        }
    }

    /// The named profiles (those any game may choose).
    pub fn named(&self) -> impl Iterator<Item = &Profile<B>> {
        self.list.iter().filter(|p| p.owner.is_none())
    }

    /// `entry`'s own profile, if it has one.
    pub fn own_of(&self, entry: &str) -> Option<&Profile<B>> {
        self.list.iter().find(|p| p.owner.as_deref() == Some(entry))
    }

    /// The id for a new profile: one past the highest, so an id is never taken twice while the highest
    /// stays. None once the ids have run out.
    fn next_id(&self) -> Option<u16> {
        self.list.iter().map(|p| p.id).max().unwrap_or(0).checked_add(1)
    }

    /// Makes `entry`'s own profile as a copy of profile `from` and returns its id. If `from` is 0 or no
    /// longer exists, the new profile has the empty body.
    pub fn make_own(&mut self, from: u16, entry: &Arc<str>) -> Option<u16> {
        let id = self.next_id()?;
        let body = self.get(from).map_or_else(B::default, |p| p.body.clone());
        self.list.push(Profile { id, name: String::new(), owner: Some(entry.clone()), body });
        self.changed(id);
        Some(id)
    }

    /// Makes `owner`'s own profile with `preset`'s body and returns its id.
    pub fn make_own_from(&mut self, preset: &Profile<B>, owner: &Arc<str>) -> Option<u16> {
        let id = self.make_own(0, owner)?;
        self.edit(id, |own| {
            own.body = preset.body.clone();
            true
        });
        Some(id)
    }

    /// Replaces profile `to`'s body with a copy of profile `from`'s. If `from` is 0 or no longer exists,
    /// `to` gets the empty body. Returns whether `to` is there.
    pub fn copy_into(&mut self, to: u16, from: u16) -> bool {
        let body = self.get(from).map_or_else(B::default, |p| p.body.clone());
        if self.get(to).is_none() {
            return false;
        }
        self.edit(to, |p| replace(&mut p.body, body));
        true
    }

    /// A name no profile has: `name`, or else `name` with a number after it. It is cleaned up the way the
    /// file name is (see clamshell-system's `store`): a `/` becomes `-`, and it is cut at 64 characters.
    pub fn unique_name(&self, name: &str) -> String {
        let name: String = name.trim().replace('/', "-").chars().take(64).collect();
        let name = name.trim();
        let taken = |n: &str| self.list.iter().any(|p| p.name == n);
        if !taken(name) {
            return name.to_owned();
        }
        (2..).map(|i| format!("{name} {i}")).find(|n| !taken(n)).unwrap_or_default()
    }

    /// Adds a new profile with the empty body, named `name` made unique, and returns its id.
    pub fn add(&mut self, name: &str) -> Option<u16> {
        let id = self.next_id()?;
        let name = self.unique_name(name);
        self.list.push(Profile::with(id, &name, B::default()));
        self.changed(id);
        Some(id)
    }

    /// Copies profile `id` under the name `name`, made unique, and returns the copy's id.
    pub fn duplicate(&mut self, id: u16, name: &str) -> Option<u16> {
        let body = self.get(id).map(|p| p.body.clone())?;
        let copy = self.add(name)?;
        self.edit(copy, |p| replace(&mut p.body, body));
        Some(copy)
    }

    /// Renames profile `id`. A name another profile has is made unique. Returns whether the profile was
    /// there.
    pub fn rename(&mut self, id: u16, name: &str) -> bool {
        if self.get(id).is_some_and(|p| p.name == name.trim()) {
            return true;
        }
        let name = self.unique_name(name);
        if self.get(id).is_none() {
            return false;
        }
        self.edit(id, |p| replace(&mut p.name, name));
        true
    }

    pub fn remove(&mut self, id: u16) -> bool {
        let before = self.list.len();
        self.list.retain(|p| p.id != id);
        self.versions.retain(|(p, _)| *p != id);
        self.list.len() != before
    }
}
