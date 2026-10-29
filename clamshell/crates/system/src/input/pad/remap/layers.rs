//! The keymap's layers: the holds that decide whether a layer key turns its layer on, layers turned on
//! and off, and a binding found in the layers on.

use super::*;

impl Remap {
    /// A hold to a layer that is still deciding is decided now if an input that layer maps is pressed. So a
    /// quick chord acts in the layer, as it would have once the hold's time was up.
    pub(super) fn decide_layer_holds(&mut self, snap: &PadSnapshot, now: Instant) {
        let mut decided = Vec::new();
        for a in &self.actuators {
            if !matches!(a.phase, Phase::Deciding(_)) {
                continue;
            }
            let Some((b, layer)) = self.find(a.input, Actuation::Hold).and_then(|b| match b.output {
                Output::Layer(layer) => Some((b, layer)),
                _ => None,
            }) else {
                continue;
            };
            let new_press = |i: Input| {
                i != a.input
                    && Actuation::ALL.into_iter().any(|act| self.keymap.get_of(layer, i, act).is_some())
                    && self.watched(i)
                    && self.pressed(i, snap)
                    && !self.actuators.iter().any(|x| x.input == i && x.phase != Phase::Up)
            };
            if Input::all().any(new_press) {
                decided.push((a.input, b.toggle));
            }
        }
        if decided.is_empty() {
            return;
        }
        for (input, toggle) in decided {
            if let Some(a) = self.actuators.iter_mut().find(|a| a.input == input) {
                a.phase = Phase::On { act: Actuation::Hold, until: None };
                a.due = None;
                if let Some(beat) = a.beats.get_mut(Actuation::Hold as usize) {
                    beat.follow(true, toggle, now);
                }
            }
        }
        self.activate(now, &[]);
    }

    /// Turns layers on and off. A binding to a layer turns it on while its actuation, or its toggle, is
    /// on. A Once layer turned on by a tap, meaning its binding went off before it was used, stays armed
    /// until the first input it maps has acted and been let go. A toggled Once layer does the same, and
    /// its toggle goes off with it. Pressing its binding again while armed turns it off. `risen` holds the
    /// actuations that came on now.
    pub(super) fn activate(&mut self, now: Instant, risen: &[(Input, Actuation)]) {
        // When an input that a Once layer maps acts, the newest such layer that is on is used.
        for &(input, act) in risen {
            if self.activations.iter().any(|x| x.input == input) {
                continue;
            }
            let keymap = &self.keymap;
            let maps = |x: &Activation| match x.binding.output {
                Output::Layer(l) => x.span != Span::Cancelled && keymap.get_of(l, input, act).is_some(),
                _ => false,
            };
            if let Some(x) = self.activations.iter_mut().rev().find(|x| maps(x))
                && x.used_by.is_none()
            {
                x.used_by = Some(input);
            }
        }
        // The bindings to layers, and whether each is on now.
        let mut seen = Vec::new();
        for a in &self.actuators {
            for act in Actuation::ALL {
                let Some(b) = self.binding_of(a.input, act).filter(|b| matches!(b.output, Output::Layer(_))) else {
                    continue;
                };
                seen.push((a.input, act, b, a.beat(act).on(&b, now)));
            }
        }
        // A binding whose input is at rest, with its actuator gone, is off.
        for x in &self.activations {
            if !self.actuators.iter().any(|a| a.input == x.input) {
                seen.push((x.input, x.act, x.binding, false));
            }
        }
        for (input, act, b, on) in seen {
            let i = self.activations.iter().position(|x| (x.input, x.act) == (input, act));
            let once = |x: &Activation| matches!(x.binding.output, Output::Layer(l) if self.keymap.layer_once(l));
            match (i.and_then(|i| self.activations.get(i)).copied(), on) {
                (None, true) => {
                    self.activations.push(Activation { input, act, binding: b, span: Span::Held, used_by: None });
                    if b.toggle {
                        self.buzz = Some(Buzz::On);
                    }
                }
                (Some(x), false) if x.span == Span::Held => {
                    if once(&x) && x.used_by.is_none() && !x.binding.toggle {
                        self.set_span(input, act, Span::Armed);
                        self.buzz = Some(Buzz::On);
                    } else {
                        self.activations.retain(|y| (y.input, y.act) != (input, act));
                        if x.binding.toggle {
                            self.buzz = Some(Buzz::Off);
                        }
                    }
                }
                (Some(x), true) if x.span == Span::Armed => {
                    self.set_span(input, act, Span::Cancelled);
                    self.buzz = Some(Buzz::Off);
                }
                (Some(x), false) if x.span == Span::Cancelled => self.activations.retain(|y| (y.input, y.act) != (input, act)),
                _ => {}
            }
        }
        // A Once layer that is armed or toggled and has been used goes off once the input that used it has
        // let go. A toggle goes off too.
        let busy = |actuators: &[Actuator], i: Input| actuators.iter().any(|a| a.input == i && (a.phase != Phase::Up || a.beats.iter().any(|b| b.raw)));
        let mut ended = Vec::new();
        for x in &self.activations {
            let once = matches!(x.binding.output, Output::Layer(l) if self.keymap.layer_once(l));
            let waits = x.span == Span::Armed || (x.span == Span::Held && x.binding.toggle);
            if let Some(user) = x.used_by
                && once
                && waits
                && !busy(&self.actuators, user)
            {
                ended.push((x.input, x.act));
            }
        }
        for (input, act) in ended {
            self.activations.retain(|y| (y.input, y.act) != (input, act));
            if let Some(beat) = self.actuators.iter_mut().find(|a| a.input == input).and_then(|a| a.beats.get_mut(act as usize)) {
                beat.latched = false;
            }
            self.buzz = Some(Buzz::Off);
        }
        // A layer whose binding is at rest goes off, unless it is armed.
        let actuators = &self.actuators;
        self.activations.retain(|x| x.span == Span::Armed || actuators.iter().any(|a| a.input == x.input));
    }

    pub(super) fn set_span(&mut self, input: Input, act: Actuation, span: Span) {
        if let Some(x) = self.activations.iter_mut().find(|x| (x.input, x.act) == (input, act)) {
            x.span = span;
        }
    }

    /// The layers that are on, in the order they came on.
    pub(super) fn layers_on(&self) -> impl DoubleEndedIterator<Item = Layer> + '_ {
        self.activations.iter().filter(|a| a.span != Span::Cancelled).filter_map(|a| if let Output::Layer(l) = a.binding.output { Some(l) } else { None })
    }

    /// `input`'s binding for `act` in force. If the input is turning a layer on, that binding. Otherwise
    /// the binding from the newest layer that is on and has one, or else Base's.
    pub(super) fn find(&self, input: Input, act: Actuation) -> Option<&Binding> {
        if let Some(a) = self.activations.iter().find(|a| a.input == input && a.act == act) {
            return Some(&a.binding);
        }
        self.keymap.lookup_in(input, act, self.layers_on())
    }
}
