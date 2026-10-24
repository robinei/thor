//! The control diagram and printed legends. These describe controls, not detected capabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Trigger,
    Bumper,
    Top,
    Stick,
    Dpad,
    Face,
    Gyro,
    Foot,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlLayout {
    /// Group heights in thousandths of the column, normalised by their sum when drawn.
    pub left: &'static [(Group, u16)],
    pub right: &'static [(Group, u16)],
    pub labels: Labels,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
    pub south: &'static str,
    pub east: &'static str,
    pub west: &'static str,
    pub north: &'static str,
    pub left_bumper: &'static str,
    pub right_bumper: &'static str,
    pub left_stick: &'static str,
    pub right_stick: &'static str,
    pub view: &'static str,
    pub menu: &'static str,
    pub auxiliary: &'static str,
    pub left_trigger: &'static str,
    pub right_trigger: &'static str,
}
impl ControlLayout {
    /// The default virtual-pad diagram also used when no physical layout is known.
    pub const fn standard() -> Self {
        Self {
            left: &[
                (Group::Trigger, 100),
                (Group::Bumper, 85),
                (Group::Top, 90),
                (Group::Stick, 300),
                (Group::Dpad, 285),
                (Group::Gyro, 70),
                (Group::Foot, 70),
            ],
            right: &[(Group::Trigger, 100), (Group::Bumper, 85), (Group::Top, 90), (Group::Face, 320), (Group::Stick, 330), (Group::Foot, 75)],
            labels: Labels {
                south: "A",
                east: "B",
                west: "X",
                north: "Y",
                left_bumper: "LB",
                right_bumper: "RB",
                left_stick: "LS",
                right_stick: "RS",
                view: "View",
                menu: "Menu",
                auxiliary: "Guide",
                left_trigger: "LT",
                right_trigger: "RT",
            },
        }
    }
    pub const fn thor() -> Self {
        Self {
            labels: Labels {
                south: "B",
                east: "A",
                west: "Y",
                north: "X",
                left_bumper: "L1",
                right_bumper: "R1",
                left_stick: "L3",
                right_stick: "R3",
                view: "Select",
                menu: "Start",
                auxiliary: "AYN",
                left_trigger: "L2",
                right_trigger: "R2",
            },
            ..Self::standard()
        }
    }
}
