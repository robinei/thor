//! Parametric colour-management objects. Invalid requests disconnect only their client.
use crate::server::Server;
use clamshell_state::session::presentation::{ColourDescription, ColourLuminance, ColourPrimaries, ColourTarget, TransferFunction};
use smithay::{
    reexports::{
        wayland_protocols::wp::color_management::v1::server::{
            wp_color_management_output_v1::{self as output, WpColorManagementOutputV1},
            wp_color_management_surface_feedback_v1::{self as feedback, WpColorManagementSurfaceFeedbackV1},
            wp_color_management_surface_v1::{self as surface, WpColorManagementSurfaceV1},
            wp_color_manager_v1::{self as manager, WpColorManagerV1},
            wp_image_description_creator_params_v1::{self as params, WpImageDescriptionCreatorParamsV1},
            wp_image_description_info_v1::{self as info, WpImageDescriptionInfoV1},
            wp_image_description_v1::{self as image, WpImageDescriptionV1},
        },
        wayland_server::{Client, DataInit, DisplayHandle, New, Resource, Weak, protocol::wl_surface::WlSurface},
    },
    wayland::{
        Dispatch2, GlobalDispatch2,
        compositor::{self, Cacheable},
    },
};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Default)]
pub(crate) struct SurfaceColour(pub Option<Arc<ColourDescription>>);
impl Cacheable for SurfaceColour {
    fn commit(&mut self, _: &DisplayHandle) -> Self {
        self.clone()
    }
    fn merge_into(self, into: &mut Self, _: &DisplayHandle) {
        *into = self;
    }
}
#[derive(Default)]
struct Attached(AtomicBool);
pub(crate) fn current(surface: &WlSurface) -> Option<Arc<ColourDescription>> {
    compositor::with_states(surface, |states| states.cached_state.get::<SurfaceColour>().current().0.clone())
}
#[derive(Clone, Copy)]
pub(crate) struct Manager;
pub(crate) struct Surface(Weak<WlSurface>);
pub(crate) struct Output(usize);
pub(crate) struct Description {
    colour: Arc<ColourDescription>,
    information: bool,
    identity: u32,
}
#[derive(Default)]
struct Parameters {
    primaries: Option<ColourPrimaries>,
    transfer: Option<TransferFunction>,
    luminance: Option<ColourLuminance>,
    target: ColourTarget,
    set: u16,
}
#[derive(Default)]
pub(crate) struct Creator(Mutex<Parameters>);

pub(crate) fn install(dh: &DisplayHandle) {
    dh.create_global::<Server, WpColorManagerV1, _>(1, Manager);
}
impl GlobalDispatch2<WpColorManagerV1, Server> for Manager {
    fn bind(&self, _: &mut Server, _: &DisplayHandle, _: &Client, object: New<WpColorManagerV1>, init: &mut DataInit<'_, Server>) {
        let manager = init.init(object, Manager);
        manager.supported_intent(manager::RenderIntent::Perceptual);
        for feature in [
            manager::Feature::Parametric,
            manager::Feature::SetPrimaries,
            manager::Feature::SetTfPower,
            manager::Feature::SetLuminances,
            manager::Feature::SetMasteringDisplayPrimaries,
            manager::Feature::ExtendedTargetVolume,
        ] {
            manager.supported_feature(feature);
        }
        for transfer in
            [manager::TransferFunction::Srgb, manager::TransferFunction::Gamma22, manager::TransferFunction::ExtLinear, manager::TransferFunction::St2084Pq]
        {
            manager.supported_tf_named(transfer);
        }
        for primaries in [manager::Primaries::Srgb, manager::Primaries::DisplayP3, manager::Primaries::Bt2020] {
            manager.supported_primaries_named(primaries);
        }
        manager.done();
    }
}
impl Dispatch2<WpColorManagerV1, Server> for Manager {
    fn request(
        &self,
        state: &mut Server,
        _: &Client,
        object: &WpColorManagerV1,
        request: manager::Request,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            manager::Request::Destroy => {}
            manager::Request::GetSurface { id, surface } => {
                let attached = compositor::with_states(&surface, |states| {
                    states.data_map.insert_if_missing_threadsafe(Attached::default);
                    states.data_map.get::<Attached>().is_some_and(|data| data.0.swap(true, Ordering::Relaxed))
                });
                if attached {
                    object.post_error(manager::Error::SurfaceExists, "surface already has colour management");
                } else {
                    init.init(id, Surface(surface.downgrade()));
                }
            }
            manager::Request::GetSurfaceFeedback { id, surface } => {
                let resource = init.init(id, Surface(surface.downgrade()));
                resource.preferred_changed(state.colour.identity);
                state.colour.feedback.retain(Resource::is_alive);
                state.colour.feedback.push(resource);
            }
            manager::Request::GetOutput { id, output } => {
                if let Some(index) = state.outputs.iter().position(|candidate| candidate.owns(&output)) {
                    let resource = init.init(id, Output(index));
                    state.colour.outputs.retain(Resource::is_alive);
                    state.colour.outputs.push(resource);
                } else {
                    init.init(id, Output(0));
                }
            }
            manager::Request::CreateParametricCreator { obj } => {
                init.init(obj, Creator::default());
            }
            _ => object.post_error(manager::Error::UnsupportedFeature, "only parametric descriptions are supported"),
        }
    }
}
impl Dispatch2<WpColorManagementSurfaceV1, Server> for Surface {
    fn request(
        &self,
        _: &mut Server,
        _: &Client,
        object: &WpColorManagementSurfaceV1,
        request: surface::Request,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Server>,
    ) {
        let Ok(surface) = self.0.upgrade() else {
            if !matches!(request, surface::Request::Destroy) {
                object.post_error(surface::Error::Inert, "surface was destroyed");
            }
            return;
        };
        match request {
            surface::Request::SetImageDescription { image_description, render_intent } => {
                if !matches!(render_intent.into_result(), Ok(manager::RenderIntent::Perceptual)) {
                    object.post_error(surface::Error::RenderIntent, "unsupported render intent");
                    return;
                }
                let Some(description) = image_description.data::<Description>() else {
                    object.post_error(surface::Error::ImageDescription, "image description is not ready");
                    return;
                };
                if description.identity == 0 {
                    object.post_error(surface::Error::ImageDescription, "image description failed");
                    return;
                }
                compositor::with_states(&surface, |states| states.cached_state.get::<SurfaceColour>().pending().0 = Some(description.colour.clone()));
            }
            surface::Request::Destroy => compositor::with_states(&surface, |states| {
                if let Some(data) = states.data_map.get::<Attached>() {
                    data.0.store(false, Ordering::Relaxed);
                }
                states.cached_state.get::<SurfaceColour>().pending().0 = None;
            }),
            surface::Request::UnsetImageDescription => {
                compositor::with_states(&surface, |states| states.cached_state.get::<SurfaceColour>().pending().0 = None)
            }
            _ => {}
        }
    }
}
fn describe(state: &mut Server, id: New<WpImageDescriptionV1>, colour: ColourDescription, information: bool, init: &mut DataInit<'_, Server>) {
    let identity = if information {
        state.colour.identity
    } else {
        let identity = state.colour.next_identity;
        state.colour.next_identity = state.colour.next_identity.wrapping_add(1).max(1);
        identity
    };
    let resource = init.init(id, Description { colour: Arc::new(colour), information, identity });
    resource.ready(identity);
}
impl Dispatch2<WpColorManagementSurfaceFeedbackV1, Server> for Surface {
    fn request(
        &self,
        state: &mut Server,
        _: &Client,
        object: &WpColorManagementSurfaceFeedbackV1,
        request: feedback::Request,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        match request {
            feedback::Request::Destroy => {}
            feedback::Request::GetPreferred { image_description } | feedback::Request::GetPreferredParametric { image_description } => {
                if self.0.upgrade().is_err() {
                    object.post_error(feedback::Error::Inert, "surface was destroyed");
                    return;
                }
                describe(state, image_description, state.colour.preferred, true, init);
            }
            _ => {}
        }
    }
}
impl Dispatch2<WpColorManagementOutputV1, Server> for Output {
    fn request(
        &self,
        state: &mut Server,
        _: &Client,
        _: &WpColorManagementOutputV1,
        request: output::Request,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let output::Request::GetImageDescription { image_description } = request {
            let colour = state.colour.descriptions.get(self.0).copied().unwrap_or_default();
            describe(state, image_description, colour, true, init);
        }
    }
}
impl Dispatch2<WpImageDescriptionV1, Server> for Description {
    fn request(
        &self,
        state: &mut Server,
        _: &Client,
        object: &WpImageDescriptionV1,
        request: image::Request,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        if let image::Request::GetInformation { information } = request {
            if !self.information {
                object.post_error(image::Error::NoInformation, "client-created description has no information object");
                return;
            }
            let resource = init.init(information, Manager);
            send_information(&resource, *self.colour);
            state.colour.information.push(resource);
        }
    }
}
impl Dispatch2<WpImageDescriptionInfoV1, Server> for Manager {
    fn request(&self, _: &mut Server, _: &Client, _: &WpImageDescriptionInfoV1, _: info::Request, _: &DisplayHandle, _: &mut DataInit<'_, Server>) {}
}
pub(crate) fn send_information(info: &WpImageDescriptionInfoV1, description: ColourDescription) {
    let [(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)] = super::math::coordinates(description.primaries);
    info.primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
    match description.transfer {
        TransferFunction::Gamma(power) => info.tf_power(power),
        TransferFunction::Srgb => info.tf_named(manager::TransferFunction::Srgb),
        TransferFunction::Linear => info.tf_named(manager::TransferFunction::ExtLinear),
        TransferFunction::Pq => info.tf_named(manager::TransferFunction::St2084Pq),
        TransferFunction::Hlg => info.tf_named(manager::TransferFunction::Hlg),
    }
    let luminance = description.luminances();
    info.luminances(luminance.min, luminance.max, luminance.reference);
    if let Some(target) = description.target {
        if let Some(primaries) = target.primaries {
            let [(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)] = super::math::coordinates(primaries);
            info.target_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
        }
        if let Some((min, max)) = target.luminance {
            info.target_luminance(min, max);
        }
        if let Some(value) = target.max_cll {
            info.target_max_cll(value);
        }
        if let Some(value) = target.max_fall {
            info.target_max_fall(value);
        }
    }
}
impl Dispatch2<WpImageDescriptionCreatorParamsV1, Server> for Creator {
    fn request(
        &self,
        state: &mut Server,
        _: &Client,
        object: &WpImageDescriptionCreatorParamsV1,
        request: params::Request,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Server>,
    ) {
        let Ok(mut data) = self.0.lock() else {
            object.post_error(params::Error::IncompleteSet, "description builder unavailable");
            return;
        };
        let bit = match &request {
            params::Request::SetTfNamed { .. } | params::Request::SetTfPower { .. } => 1,
            params::Request::SetPrimariesNamed { .. } | params::Request::SetPrimaries { .. } => 2,
            params::Request::SetLuminances { .. } => 4,
            params::Request::SetMasteringDisplayPrimaries { .. } => 8,
            params::Request::SetMasteringLuminance { .. } => 16,
            params::Request::SetMaxCll { .. } => 32,
            params::Request::SetMaxFall { .. } => 64,
            _ => 0,
        };
        if bit != 0 && data.set & bit != 0 {
            object.post_error(params::Error::AlreadySet, "parameter already set");
            return;
        }
        data.set |= bit;
        match request {
            params::Request::SetTfNamed { tf } => {
                data.transfer = match tf.into_result() {
                    Ok(manager::TransferFunction::Srgb) => Some(TransferFunction::Srgb),
                    Ok(manager::TransferFunction::Gamma22) => Some(TransferFunction::Gamma(22000)),
                    Ok(manager::TransferFunction::ExtLinear) => Some(TransferFunction::Linear),
                    Ok(manager::TransferFunction::St2084Pq) => Some(TransferFunction::Pq),
                    _ => None,
                };
                if data.transfer.is_none() {
                    object.post_error(params::Error::InvalidTf, "unsupported transfer function");
                }
            }
            params::Request::SetTfPower { eexp } => {
                if !(10000..=100000).contains(&eexp) {
                    object.post_error(params::Error::InvalidTf, "power outside supported range");
                } else {
                    data.transfer = Some(TransferFunction::Gamma(eexp));
                }
            }
            params::Request::SetPrimariesNamed { primaries } => {
                data.primaries = match primaries.into_result() {
                    Ok(manager::Primaries::Srgb) => Some(ColourPrimaries::Srgb),
                    Ok(manager::Primaries::DisplayP3) => Some(ColourPrimaries::DisplayP3),
                    Ok(manager::Primaries::Bt2020) => Some(ColourPrimaries::Bt2020),
                    _ => None,
                };
                if data.primaries.is_none() {
                    object.post_error(params::Error::InvalidPrimariesNamed, "unsupported named primaries");
                }
            }
            params::Request::SetPrimaries { r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y } => {
                data.primaries = Some(ColourPrimaries::Custom([(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)]))
            }
            params::Request::SetMasteringDisplayPrimaries { r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y } => {
                data.target.primaries = Some(ColourPrimaries::Custom([(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)]))
            }
            params::Request::SetLuminances { min_lum, max_lum, reference_lum } => {
                if u64::from(max_lum) * 10000 <= u64::from(min_lum) || u64::from(reference_lum) * 10000 <= u64::from(min_lum) {
                    object.post_error(params::Error::InvalidLuminance, "luminance range must increase");
                } else {
                    data.luminance = Some(ColourLuminance { min: min_lum, max: max_lum, reference: reference_lum });
                }
            }
            params::Request::SetMasteringLuminance { min_lum, max_lum } => {
                if u64::from(max_lum) * 10000 <= u64::from(min_lum) {
                    object.post_error(params::Error::InvalidLuminance, "mastering luminance range must increase");
                } else {
                    data.target.luminance = Some((min_lum, max_lum));
                }
            }
            params::Request::SetMaxCll { max_cll } => data.target.max_cll = Some(max_cll),
            params::Request::SetMaxFall { max_fall } => data.target.max_fall = Some(max_fall),
            params::Request::Create { image_description } => {
                let (Some(primaries), Some(transfer)) = (data.primaries, data.transfer) else {
                    object.post_error(params::Error::IncompleteSet, "primaries and transfer function are required");
                    return;
                };
                if data.target.max_cll.zip(data.target.max_fall).is_some_and(|(cll, fall)| fall > cll) {
                    object.post_error(params::Error::InvalidLuminance, "frame average exceeds content peak");
                    return;
                }
                let mut colour = ColourDescription {
                    primaries,
                    transfer,
                    luminance: data.luminance,
                    target: (data.target != ColourTarget::default()).then_some(data.target),
                };
                if transfer == TransferFunction::Pq
                    && let Some(luminance) = &mut colour.luminance
                {
                    luminance.max = 10000 + luminance.min / 10000;
                }
                if super::math::matrix(primaries, ColourPrimaries::Bt2020).is_none() {
                    let resource = init.init(image_description, Description { colour: Arc::new(colour), information: false, identity: 0 });
                    resource.failed(image::Cause::Unsupported, "unsupported primary matrix or white point".into());
                } else {
                    describe(state, image_description, colour, false, init);
                }
            }
            _ => {}
        }
    }
}
