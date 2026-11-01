//! What is stored about the library's entries, applied as it is read.

use super::*;

/// Applies what is stored about `library`'s entries: their plays, whether each is a favourite, what was
/// fetched about them if its images still exist, and the positions, lengths and frames of mpv's videos.
/// Returns what to fetch; `hows` says where each entry's metadata comes from.
pub(super) fn known(library: &mut Library, hows: Vec<(usize, How)>, sources: &Sources) -> Vec<Wanted> {
    let mut metas = std::collections::HashMap::new();
    let mut probes = std::collections::HashMap::new();
    if let Some(art) = sources.art.as_deref() {
        let cache = crate::library::meta::cache::Cache::open(art);
        metas = cache.metas();
        probes = cache.probes();
    }
    if let Some(state) = sources.state.as_deref() {
        let store = crate::store::Store::open(state);
        let plays = store.plays();
        let favourites = store.favourites();
        for g in library.entries_mut() {
            if let Some(p) = plays.get(&*g.id) {
                g.plays = *p;
            }
            g.favourite = favourites.contains(&*g.id);
        }
        let mut away: Vec<String> = favourites.into_iter().filter(|id| library.away.iter().any(|p| id.starts_with(p.as_str()))).collect();
        away.sort();
        library.away_favourites = away;
    }
    let now = crate::time::clock::unix_secs();
    let mut wanted = Vec::new();
    for (i, how) in hows {
        let Some(g) = library.entries().get(i) else {
            continue;
        };
        let fetched = metas.get(&*g.id);
        let gone = |p: &Option<std::path::PathBuf>| p.as_ref().is_some_and(|p| !p.exists());
        let want = match fetched {
            None => true,
            Some(f) => {
                f.source != how.source()
                    || gone(&f.meta.cover)
                    || gone(&f.meta.details.shot)
                    || (f.meta == Meta::default() && now.saturating_sub(f.at) > RETRY_MISS)
            }
        };
        if want {
            wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: g.file.clone(), how, art: g.box_front.is_some() });
        } else if let Some(f) = fetched {
            library.apply_meta(i, &f.meta);
        }
    }
    // For mpv's videos: where mpv left each, its length and a frame of it. The length and frame are probed
    // once, then kept.
    for g in library.entries_mut() {
        let Some(file) = g.mpv_file().map(std::path::Path::to_path_buf) else {
            continue;
        };
        let kept = crate::launch::mpv::position(&sources.watch_later, &file);
        (g.playback.position, g.playback.at) = (kept.map(|(s, _)| s), kept.map_or(0, |(_, at)| at));
        let probe = probes.get(&*g.id).filter(|p| p.frame.as_ref().is_none_or(|f| f.exists()));
        match probe {
            Some(p) => {
                g.playback.duration = p.seconds;
                if g.details.shot.is_none() {
                    g.details.shot.clone_from(&p.frame);
                }
            }
            None => wanted.push(Wanted { id: g.id.clone(), key: g.key.clone(), title: g.title.clone(), file: Some(file), how: How::Duration, art: true }),
        }
    }
    // A folder of videos without art uses the frame of its first video that has one.
    for i in 0..library.entries().len() {
        let bare = library.entries().get(i).is_some_and(|g| g.is_group() && g.box_front.is_none() && g.details.shot.is_none());
        if bare
            && let Some(frame) = library.first_frame(i)
            && let Some(g) = library.entries_mut().get_mut(i)
        {
            g.details.shot = Some(frame);
        }
    }
    wanted
}
