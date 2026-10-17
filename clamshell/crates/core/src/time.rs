//! The time tab: the time zone as the system has it (systemd's timedated, read by the system side while
//! the tab is open, and after a change), and choosing another: the regions (Europe, America, ...) and UTC,
//! then a region's zones, the one in force marked. A zone tapped is set at once; the clock follows.

use crate::State;
use crate::effect::Effect;
use crate::scroll::ListId;

/// The time zone as the system has it, and the ones to choose from (tzdata's names, "Europe/Oslo").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TimeZones {
    pub current: Option<String>,
    pub zones: Vec<String>,
    /// Why reading or setting it failed, until a reading that works.
    pub error: Option<String>,
}

/// The regions the zones are chosen by (tzdata's first parts, without the old and the administrative
/// ones: Etc, US, Canada...); UTC is a row of its own after them.
pub const REGIONS: [&str; 10] = ["Africa", "America", "Antarctica", "Arctic", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific"];

/// A row of the time tab's list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeRow<'a> {
    Region(&'a str),
    Utc,
    /// A zone of the region shown, by its full name.
    Zone(&'a str),
}

impl TimeZones {
    /// A region's zones, in tzdata's order.
    fn of<'a>(&'a self, region: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.zones.iter().map(String::as_str).filter(move |z| in_region(z, region))
    }
}

/// Whether `zone` ("Europe/Oslo", "America/Argentina/Salta") is in `region`.
pub fn in_region(zone: &str, region: &str) -> bool {
    zone.strip_prefix(region).is_some_and(|rest| rest.starts_with('/'))
}

impl State {
    pub fn time_zones(&self) -> &TimeZones {
        &self.sys.time
    }

    /// The region whose zones the tab shows, if one is open.
    pub fn time_region(&self) -> Option<&'static str> {
        self.bottom.time_region.and_then(|i| REGIONS.get(usize::from(i)).copied())
    }

    pub(crate) fn time_rows(&self) -> usize {
        match self.time_region() {
            None => REGIONS.len() + 1,
            Some(region) => self.sys.time.of(region).count(),
        }
    }

    pub fn time_row(&self, i: usize) -> Option<TimeRow<'_>> {
        match self.time_region() {
            None => REGIONS.get(i).map(|r| TimeRow::Region(r)).or((i == REGIONS.len()).then_some(TimeRow::Utc)),
            Some(region) => self.sys.time.of(region).nth(i).map(TimeRow::Zone),
        }
    }

    /// Whether a row is the zone in force, or the region it is in.
    pub fn time_row_current(&self, row: TimeRow) -> bool {
        let Some(current) = self.sys.time.current.as_deref() else {
            return false;
        };
        match row {
            TimeRow::Region(region) => in_region(current, region),
            TimeRow::Utc => current == "UTC",
            TimeRow::Zone(zone) => current == zone,
        }
    }

    /// A tap on a row: a region opens (scrolled to the zone in force if it is there), UTC or a zone is set.
    pub(crate) fn on_time_row(&mut self, i: usize) -> Vec<Effect> {
        let region = match self.time_row(i) {
            Some(TimeRow::Region(_)) => u8::try_from(i).ok(),
            Some(TimeRow::Utc) => return vec![Effect::SetTimeZone("UTC".to_owned())],
            Some(TimeRow::Zone(zone)) => return vec![Effect::SetTimeZone(zone.to_owned())],
            None => return vec![],
        };
        self.bottom.time_region = region;
        let current = self.sys.time.current.as_deref();
        let at = self.time_region().and_then(|region| self.sys.time.of(region).position(|z| Some(z) == current));
        self.show_time_row(at.unwrap_or(0));
        vec![]
    }

    /// Back from a region's zones to the regions, that region shown.
    pub(crate) fn time_back(&mut self) {
        if let Some(i) = self.bottom.time_region.take() {
            self.show_time_row(usize::from(i));
        }
    }

    fn show_time_row(&mut self, i: usize) {
        let len = self.time_rows();
        if let Some(scroll) = self.scroll_mut(ListId::Time) {
            // (From the top: another region's place means nothing here.)
            scroll.scroll_by(-scroll.offset(), len);
            scroll.show(i, len);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::event::Reading;
    use crate::{Event, Id, Init, Tab};

    fn state() -> State {
        let mut s = State::new(Init::default());
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Tab(Tab::Time))));
        let zones = ["Africa/Cairo", "America/Argentina/Salta", "America/New_York", "Europe/London", "Europe/Oslo", "UTC"];
        let time = TimeZones { current: Some("Europe/Oslo".to_owned()), zones: zones.map(str::to_owned).to_vec(), error: None };
        s.apply(Event::Reading(Reading::Time(time)));
        s
    }

    fn tap(s: &mut State, i: u16) -> Vec<Effect> {
        s.apply(Event::Pointer(crate::PointerEvent::Activate(ListId::Time.row(i)))).1
    }

    #[test]
    fn regions_then_their_zones_and_a_zone_tapped_is_set() {
        let mut s = state();
        assert_eq!(s.time_rows(), REGIONS.len() + 1, "the regions and UTC");
        assert!(s.time_row_current(TimeRow::Region("Europe")));
        assert!(!s.time_row_current(TimeRow::Region("America")));
        assert!(tap(&mut s, 1).is_empty(), "America opens");
        assert_eq!(s.time_region(), Some("America"));
        assert_eq!(
            (s.time_row(0), s.time_row(1), s.time_row(2)),
            (Some(TimeRow::Zone("America/Argentina/Salta")), Some(TimeRow::Zone("America/New_York")), None)
        );
        assert_eq!(tap(&mut s, 1), vec![Effect::SetTimeZone("America/New_York".to_owned())]);
        s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::TimeRegions)));
        assert_eq!(s.time_region(), None, "back");
        assert_eq!(tap(&mut s, REGIONS.len() as u16), vec![Effect::SetTimeZone("UTC".to_owned())]);
    }

    #[test]
    fn the_tab_reads_the_time_zone_while_open() {
        let mut s = State::new(Init::default());
        let (_, effects) = s.apply(Event::Pointer(crate::PointerEvent::Activate(Id::Tab(Tab::Time))));
        assert_eq!(effects, vec![Effect::Watch(crate::Watch::Time, true)]);
    }
}
