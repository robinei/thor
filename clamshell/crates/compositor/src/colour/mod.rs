//! Parametric colour conversion. Linear BT.2020 with one unit at reference white is the working space.
pub(crate) mod math;

pub(crate) mod host;
pub(crate) mod protocol;
pub(crate) mod render;
use clamshell_state::session::presentation::ColourDescription;
use smithay::reexports::wayland_protocols::wp::color_management::v1::server::{
    wp_color_management_output_v1::WpColorManagementOutputV1, wp_color_management_surface_feedback_v1::WpColorManagementSurfaceFeedbackV1,
};
pub(crate) struct State {
    pub information: Vec<smithay::reexports::wayland_protocols::wp::color_management::v1::server::wp_image_description_info_v1::WpImageDescriptionInfoV1>,
    pub surfaces: std::collections::HashMap<smithay::backend::renderer::element::Id, std::sync::Arc<ColourDescription>>,
    pub revision: u64,
    pub descriptions: [ColourDescription; 2],
    pub preferred: ColourDescription,
    /// The identities clients are given for each screen's description; the top's is the preferred one's.
    /// Equal only while the descriptions are.
    pub identities: [u32; 2],
    pub next_identity: u32,
    pub outputs: Vec<WpColorManagementOutputV1>,
    pub feedback: Vec<WpColorManagementSurfaceFeedbackV1>,
}
impl Default for State {
    fn default() -> Self {
        Self {
            information: Vec::new(),
            surfaces: Default::default(),
            revision: 0,
            descriptions: [ColourDescription::default(); 2],
            preferred: ColourDescription::default(),
            identities: [1, 1],
            next_identity: 2,
            outputs: Vec::new(),
            feedback: Vec::new(),
        }
    }
}

impl State {
    /// Each screen's description (the top's, the second's), which clients are told for its output. The
    /// preferred one, which surfaces are told, is the top's, where games are. Returns whether it changed.
    pub fn set_outputs(&mut self, descriptions: [ColourDescription; 2]) -> bool {
        use smithay::reexports::wayland_server::Resource;
        if self.descriptions == descriptions && self.preferred == descriptions[0] {
            return false;
        }
        self.preferred = descriptions[0];
        self.descriptions = descriptions;
        let mut next = || {
            let identity = self.next_identity;
            self.next_identity = self.next_identity.wrapping_add(1).max(1);
            identity
        };
        let top = next();
        let [first, second] = descriptions;
        self.identities = [top, if second == first { top } else { next() }];
        self.revision = self.revision.wrapping_add(1);
        self.outputs.retain(Resource::is_alive);
        self.feedback.retain(Resource::is_alive);
        for output in &self.outputs {
            output.image_description_changed();
        }
        for feedback in &self.feedback {
            feedback.preferred_changed(self.identities[0]);
        }
        true
    }

    /// Screen `down`'s description (the second's, or the top's).
    pub fn screen(&self, down: bool) -> ColourDescription {
        let [top, bottom] = self.descriptions;
        if down { bottom } else { top }
    }
}

#[cfg(test)]
mod tests;

impl crate::server::Server {
    /// The declared content volume, anchored at the PQ output's reference white. Unknown metadata
    /// conservatively uses the primary volume. Hidden windows cannot change the presented volume.
    pub fn colour_volume(&self) -> (u32, u32) {
        use smithay::backend::renderer::element::Id;
        let (mut peak, mut average) = (203, 203);
        for client in self.windows.iter().filter(|client| client.mapped && self.visible(client.id)) {
            client.window.with_surfaces(|surface, _| {
                let Some(description) = self.colour.surfaces.get(&Id::from_wayland_resource(surface)) else { return };
                let luminance = description.luminances();
                let target = description.target.unwrap_or_default();
                let maximum = target.max_cll.or(target.luminance.map(|(_, maximum)| maximum)).unwrap_or(luminance.max);
                let mean = target.max_fall.unwrap_or(maximum);
                let anchored = |value: u32| (u64::from(value) * 203).div_ceil(u64::from(luminance.reference.max(1))).min(10000) as u32;
                peak = peak.max(anchored(maximum));
                average = average.max(anchored(mean));
            });
        }
        (peak, average.min(peak))
    }
}
