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
    pub identity: u32,
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
            identity: 1,
            next_identity: 2,
            outputs: Vec::new(),
            feedback: Vec::new(),
        }
    }
}

impl State {
    pub fn set_output(&mut self, description: ColourDescription) {
        use smithay::reexports::wayland_server::Resource;
        if self.preferred == description {
            return;
        }
        self.preferred = description;
        self.descriptions = [description; 2];
        self.identity = self.next_identity;
        self.next_identity = self.next_identity.wrapping_add(1).max(1);
        self.revision = self.revision.wrapping_add(1);
        self.outputs.retain(Resource::is_alive);
        self.feedback.retain(Resource::is_alive);
        for output in &self.outputs {
            output.image_description_changed();
        }
        for feedback in &self.feedback {
            feedback.preferred_changed(self.identity);
        }
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
