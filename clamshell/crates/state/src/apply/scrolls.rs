//! The lists' scrolling: scrolled or laid out, moved to show the pad's focus, and kept on their top items
//! as their contents change.

use crate::{Id, ListEvent, ListId, Scroll, State};

impl State {
    fn scroll_mut(&mut self, list: ListId) -> Option<&mut Scroll> {
        match list {
            ListId::LibraryView => self.library_view.scroll_mut(),
            _ => self.scrolls.get_mut(list),
        }
    }

    /// A list scrolled or was laid out.
    pub(crate) fn on_list(&mut self, event: ListEvent) {
        match event {
            ListEvent::Scroll(list, rows) => self.move_list(list, |s, len| s.scroll_by(rows, len)),
            ListEvent::Visible(list, rows) => self.move_list(list, |s, len| s.set_visible(rows, len)),
        }
    }

    /// The pad's focus moved to `id`. If it is a list row, the list scrolls to show it.
    pub(crate) fn show_focused(&mut self, id: &Id) {
        if let Some((list, i)) = self.lists().of(id) {
            self.move_list(list, |s, len| s.show(i, len));
        }
    }

    /// Moves `list` by `f`, given its scroll and how many rows it has. If it moved, the item now at the top
    /// of its view is remembered, to keep there when the list changes.
    fn move_list(&mut self, list: ListId, f: impl FnOnce(&mut Scroll, usize) -> bool) {
        let len = self.lists().len(list);
        if !self.scroll_mut(list).is_some_and(|s| f(s, len)) {
            return;
        }
        let lists = self.lists();
        let key = lists.rows(list).key(lists.scroll(list).top());
        if let Some(s) = self.scroll_mut(list) {
            s.anchor(key);
        }
    }

    /// Runs after every event, since any may change the lists' contents. Each scrolled list keeps its top
    /// item where it is, while that item is still there, and stays within its ends.
    pub(crate) fn follow_lists(&mut self) {
        // The lists are built once (each build works out the subject and the app in front), and looked at
        // for every list before any scroll changes.
        let lists = self.lists();
        let follow = ListId::ALL.map(|list| {
            let found = lists.scroll(list).anchored().and_then(|key| lists.rows(list).find(key));
            (found, lists.len(list))
        });
        for (list, (found, len)) in ListId::ALL.into_iter().zip(follow) {
            if let Some(s) = self.scroll_mut(list) {
                s.follow(found, len);
            }
        }
    }
}
