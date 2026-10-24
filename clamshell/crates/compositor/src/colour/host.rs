//! A separate queue on winit's connection; winit remains the sole reader of the host socket.
use clamshell_state::session::presentation::{ColourDescription, ColourLuminance, ColourPrimaries, ColourTarget, TransferFunction};
use smithay::reexports::{
    wayland_protocols::wp::color_management::v1::client::{
        wp_color_management_surface_feedback_v1::{self as feedback, WpColorManagementSurfaceFeedbackV1},
        wp_color_management_surface_v1::WpColorManagementSurfaceV1,
        wp_color_manager_v1::{self as manager, WpColorManagerV1},
        wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1,
        wp_image_description_info_v1::{self as info, WpImageDescriptionInfoV1},
        wp_image_description_v1::{self as image, WpImageDescriptionV1},
    },
    winit::window::Window,
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop,
    protocol::{wl_registry, wl_surface::WlSurface},
};

pub(crate) struct Host {
    connection: Connection,
    queue: EventQueue<Data>,
    data: Data,
}
struct Data {
    surface: WlSurface,
    manager: Option<WpColorManagerV1>,
    control: Option<WpColorManagementSurfaceV1>,
    feedback: Option<WpColorManagementSurfaceFeedbackV1>,
    description: Option<WpImageDescriptionV1>,
    source: ColourDescription,
    preferred: Option<ColourDescription>,
    changed: bool,
    generation: u64,
    parametric: bool,
    primaries: bool,
    power: bool,
    luminances: bool,
    perceptual: bool,
    mastering: bool,
    pq: bool,
    requested: Option<ColourDescription>,
    output_generation: u64,
    active: Option<ColourDescription>,
    valid_transfer: bool,
}
#[derive(Clone, Copy)]
enum Image {
    Preferred(u64),
    Output(ColourDescription, u64),
}
#[derive(Clone, Copy)]
struct Generation(u64);
impl Host {
    pub(crate) fn new(window: &dyn Window) -> Option<Self> {
        let window = window.cast_ref::<winit_wayland::Window>()?;
        let surface = window.surface().clone();
        let connection = Connection::from_backend(surface.backend().upgrade()?);
        let queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        Some(Self {
            connection,
            queue,
            data: Data {
                surface,
                manager: None,
                control: None,
                feedback: None,
                description: None,
                source: ColourDescription::default(),
                preferred: None,
                changed: false,
                generation: 0,
                parametric: false,
                primaries: false,
                power: false,
                luminances: false,
                perceptual: false,
                mastering: false,
                pq: false,
                requested: None,
                output_generation: 0,
                active: None,
                valid_transfer: false,
            },
        })
    }
    pub fn encoding(&self) -> Option<ColourDescription> {
        self.data.active
    }

    /// Dispatches only events winit has already read. It never performs a socket read or round trip.
    pub(crate) fn turn(&mut self) -> Result<Option<ColourDescription>, String> {
        self.queue.dispatch_pending(&mut self.data).map_err(|error| error.to_string())?;
        if let Err(error) = self.connection.flush()
            && !matches!(&error,wayland_client::backend::WaylandError::Io(error) if error.kind()==std::io::ErrorKind::WouldBlock)
        {
            return Err(error.to_string());
        }
        Ok(std::mem::take(&mut self.data.changed).then_some(self.data.preferred).flatten())
    }
    /// A PQ signal retains highlight headroom even when the host prefers a bounded gamma signal.
    /// Primaries come from the preferred description; reference white is our encoding policy.
    pub fn planned_encoding(&self, peak: u32, average: u32) -> Option<ColourDescription> {
        let preferred = self.data.preferred?;
        if self.data.pq && self.data.mastering {
            Some(ColourDescription {
                primaries: preferred.primaries,
                transfer: TransferFunction::Pq,
                luminance: Some(ColourLuminance { min: 0, max: 10000, reference: 203 }),
                target: Some(ColourTarget {
                    primaries: Some(preferred.primaries),
                    luminance: Some((0, peak.clamp(203, 10000))),
                    max_cll: Some(peak.clamp(203, 10000)),
                    max_fall: Some(average.clamp(203, peak.clamp(203, 10000))),
                }),
            })
        } else {
            Some(preferred)
        }
    }
    /// What clients are told the output can show: our PQ encoding, with the host's peak and average
    /// anchored to our reference white. Advertising the content's current volume instead (as the encoding
    /// sent to the host does) would make clients tone-map down to it and never declare more.
    pub fn advertised(&self) -> Option<ColourDescription> {
        Some(advertise(self.data.preferred?, self.data.pq && self.data.mastering))
    }
    /// The selected output encoding is described before the host surface's next commit.
    pub(crate) fn describe(&mut self, description: ColourDescription) -> bool {
        if self.data.requested == Some(description) {
            return true;
        }
        let data = &self.data;
        let Some(manager) = &data.manager else { return false };
        if (matches!(description.primaries, ColourPrimaries::Custom(_)) && !data.primaries)
            || matches!(description.transfer, TransferFunction::Hlg)
            || (matches!(description.transfer,TransferFunction::Gamma(power) if power!=22000) && !data.power)
            || (description.target.is_some() && !data.mastering)
            || !data.parametric
            || !data.perceptual
            || !data.luminances
        {
            return false;
        }
        let qh = self.queue.handle();
        let creator = manager.create_parametric_creator(&qh, ());
        match description.primaries {
            ColourPrimaries::Srgb => creator.set_primaries_named(manager::Primaries::Srgb),
            ColourPrimaries::DisplayP3 => creator.set_primaries_named(manager::Primaries::DisplayP3),
            ColourPrimaries::Bt2020 => creator.set_primaries_named(manager::Primaries::Bt2020),
            ColourPrimaries::Custom([(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)]) if data.primaries => {
                creator.set_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y)
            }
            _ => return false,
        }
        match description.transfer {
            TransferFunction::Srgb => creator.set_tf_named(manager::TransferFunction::Srgb),
            TransferFunction::Pq => creator.set_tf_named(manager::TransferFunction::St2084Pq),
            TransferFunction::Linear => creator.set_tf_named(manager::TransferFunction::ExtLinear),
            TransferFunction::Gamma(22000) if !data.power => creator.set_tf_named(manager::TransferFunction::Gamma22),
            TransferFunction::Gamma(exponent) if data.power => creator.set_tf_power(exponent),
            _ => return false,
        }
        let luminance = description.luminances();
        creator.set_luminances(luminance.min, luminance.max, luminance.reference);
        if let Some(target) = description.target {
            if let Some(primaries) = target.primaries {
                let [(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)] = primaries.coordinates();
                creator.set_mastering_display_primaries(r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y);
            }
            if let Some((min, max)) = target.luminance {
                creator.set_mastering_luminance(min, max);
            }
            if let Some(value) = target.max_cll {
                creator.set_max_cll(value);
            }
            if let Some(value) = target.max_fall {
                creator.set_max_fall(value);
            }
        }
        self.data.output_generation = self.data.output_generation.wrapping_add(1);
        self.data.requested = Some(description);
        creator.create(&qh, Image::Output(description, self.data.output_generation));
        true
    }
}
/// The client-facing description for a host preference; `hdr` when the host takes PQ with mastering
/// metadata. Otherwise the host's own description is passed on.
fn advertise(preferred: ColourDescription, hdr: bool) -> ColourDescription {
    if !hdr {
        return preferred;
    }
    let luminance = preferred.luminances();
    let anchored = |value: u32| (u64::from(value) * 203).div_ceil(u64::from(luminance.reference.max(1))).min(10000) as u32;
    let target = preferred.target.unwrap_or_default();
    let peak = anchored(target.luminance.map(|(_, max)| max).or(target.max_cll).unwrap_or(luminance.max)).max(203);
    let average = target.max_fall.map_or(peak, anchored).clamp(203, peak);
    ColourDescription {
        primaries: preferred.primaries,
        transfer: TransferFunction::Pq,
        luminance: Some(ColourLuminance { min: 0, max: 10000, reference: 203 }),
        target: Some(ColourTarget {
            primaries: Some(target.primaries.unwrap_or(preferred.primaries)),
            luminance: Some((target.luminance.map_or(0, |(min, _)| min), peak)),
            max_cll: Some(peak),
            max_fall: Some(average),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clients_are_told_the_hosts_headroom_not_the_contents_volume() {
        // KWin on an HDR monitor: gamma 2.2 signal, 1400 nits peak, 951 average, reference white 218.
        let host = ColourDescription {
            primaries: ColourPrimaries::Bt2020,
            transfer: TransferFunction::Gamma(22000),
            luminance: Some(ColourLuminance { min: 0, max: 1400, reference: 218 }),
            target: Some(ColourTarget { primaries: None, luminance: Some((50, 1400)), max_cll: None, max_fall: Some(951) }),
        };
        let advertised = advertise(host, true);
        assert_eq!(advertised.transfer, TransferFunction::Pq);
        assert_eq!(advertised.luminance, Some(ColourLuminance { min: 0, max: 10000, reference: 203 }));
        let target = advertised.target.unwrap_or_default();
        // Anchored to our reference white: 1400 * 203 / 218 and 951 * 203 / 218, rounded up.
        assert_eq!((target.luminance, target.max_cll, target.max_fall), (Some((50, 1304)), Some(1304), Some(886)));
        assert_eq!(target.primaries, Some(ColourPrimaries::Bt2020));
        // An SDR host, or one without PQ, is passed on as it is.
        assert_eq!(advertise(host, false), host);
    }
}

impl Data {
    fn request_preferred(&mut self, qh: &QueueHandle<Self>) {
        let Some(feedback) = &self.feedback else { return };
        self.generation = self.generation.wrapping_add(1);
        if let Some(description) = self.description.take() {
            description.destroy();
        }
        self.source = ColourDescription::default();
        self.valid_transfer = false;
        self.description = Some(feedback.get_preferred_parametric(qh, Image::Preferred(self.generation)));
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for Data {
    fn event(data: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event
            && interface == WpColorManagerV1::interface().name
            && version >= 1
            && data.manager.is_none()
        {
            let manager: WpColorManagerV1 = registry.bind(name, 1, qh, ());
            data.control = Some(manager.get_surface(&data.surface, qh, ()));
            data.feedback = Some(manager.get_surface_feedback(&data.surface, qh, ()));
            data.manager = Some(manager);
        }
    }
}
impl Dispatch<WpColorManagerV1, ()> for Data {
    fn event(data: &mut Self, _: &WpColorManagerV1, event: manager::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            manager::Event::SupportedFeature { feature } => match feature.into_result() {
                Ok(manager::Feature::Parametric) => data.parametric = true,
                Ok(manager::Feature::SetPrimaries) => data.primaries = true,
                Ok(manager::Feature::SetTfPower) => data.power = true,
                Ok(manager::Feature::SetLuminances) => data.luminances = true,
                Ok(manager::Feature::SetMasteringDisplayPrimaries) => data.mastering = true,
                _ => {}
            },
            manager::Event::SupportedTfNamed { tf } if matches!(tf.into_result(), Ok(manager::TransferFunction::St2084Pq)) => data.pq = true,
            manager::Event::Done if data.parametric => data.request_preferred(qh),
            manager::Event::SupportedIntent { render_intent } if matches!(render_intent.into_result(), Ok(manager::RenderIntent::Perceptual)) => {
                data.perceptual = true
            }
            _ => {}
        }
    }
}
impl Dispatch<WpColorManagementSurfaceFeedbackV1, ()> for Data {
    fn event(data: &mut Self, _: &WpColorManagementSurfaceFeedbackV1, event: feedback::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if data.parametric && matches!(event, feedback::Event::PreferredChanged { .. }) {
            data.request_preferred(qh);
        }
    }
}
impl Dispatch<WpImageDescriptionV1, Image> for Data {
    fn event(data: &mut Self, object: &WpImageDescriptionV1, event: image::Event, kind: &Image, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            image::Event::Ready { .. } => match *kind {
                Image::Preferred(generation) if generation == data.generation => {
                    object.get_information(qh, Generation(generation));
                }
                Image::Output(description, generation) if generation == data.output_generation => {
                    data.active = Some(description);
                    if let Some(control) = &data.control {
                        control.set_image_description(object, manager::RenderIntent::Perceptual);
                    }
                    object.destroy();
                }
                Image::Output(..) => object.destroy(),
                _ => {}
            },
            image::Event::Failed { msg, .. } => {
                log::warn!("host colour description: {msg}");
                if matches!(kind, Image::Output(..)) {
                    object.destroy();
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<WpImageDescriptionInfoV1, Generation> for Data {
    fn event(data: &mut Self, _: &WpImageDescriptionInfoV1, event: info::Event, generation: &Generation, _: &Connection, _: &QueueHandle<Self>) {
        if generation.0 != data.generation {
            return;
        }
        let target = || ColourTarget::default();
        match event {
            info::Event::Primaries { r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y } => {
                data.source.primaries = ColourPrimaries::Custom([(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)])
            }
            info::Event::PrimariesNamed { primaries } => match primaries.into_result() {
                Ok(manager::Primaries::Srgb) => data.source.primaries = ColourPrimaries::Srgb,
                Ok(manager::Primaries::DisplayP3) => data.source.primaries = ColourPrimaries::DisplayP3,
                Ok(manager::Primaries::Bt2020) => data.source.primaries = ColourPrimaries::Bt2020,
                _ => {}
            },
            info::Event::TfNamed { tf } => {
                data.valid_transfer = true;
                match tf.into_result() {
                    Ok(manager::TransferFunction::Srgb) => data.source.transfer = TransferFunction::Srgb,
                    Ok(manager::TransferFunction::Gamma22) => data.source.transfer = TransferFunction::Gamma(22000),
                    Ok(manager::TransferFunction::ExtLinear) => data.source.transfer = TransferFunction::Linear,
                    Ok(manager::TransferFunction::St2084Pq) => data.source.transfer = TransferFunction::Pq,
                    _ => data.valid_transfer = false,
                }
            }
            info::Event::TfPower { eexp } => {
                data.source.transfer = TransferFunction::Gamma(eexp);
                data.valid_transfer = true;
            }
            info::Event::Luminances { min_lum, max_lum, reference_lum } => {
                data.source.luminance = Some(ColourLuminance { min: min_lum, max: max_lum, reference: reference_lum })
            }
            info::Event::TargetPrimaries { r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y } => {
                data.source.target.get_or_insert_with(target).primaries = Some(ColourPrimaries::Custom([(r_x, r_y), (g_x, g_y), (b_x, b_y), (w_x, w_y)]))
            }
            info::Event::TargetLuminance { min_lum, max_lum } => data.source.target.get_or_insert_with(target).luminance = Some((min_lum, max_lum)),
            info::Event::TargetMaxCll { max_cll } => data.source.target.get_or_insert_with(target).max_cll = Some(max_cll),
            info::Event::TargetMaxFall { max_fall } => data.source.target.get_or_insert_with(target).max_fall = Some(max_fall),
            info::Event::Done => {
                if data.valid_transfer
                    && super::math::matrix(data.source.primaries, ColourPrimaries::Bt2020).is_some()
                    && super::math::decode(0.5, data.source).is_some()
                {
                    data.changed = data.preferred != Some(data.source);
                    data.preferred = Some(data.source);
                } else {
                    log::warn!("host preferred colour description cannot be represented");
                }
            }
            _ => {}
        }
    }
}
delegate_noop!(Data: ignore WpColorManagementSurfaceV1);
delegate_noop!(Data: ignore WpImageDescriptionCreatorParamsV1);
