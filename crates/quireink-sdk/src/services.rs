//! What plugins offer each other: services, and points to contribute to.
//!
//! **Services.** A plugin publishes a value under an id in its namespace
//! ([`Registrar::add_service`](crate::Registrar::add_service)); others find
//! it with [`Services::get`] by that id and its type. The type comes from
//! the publishing plugin's crate, usually a trait object, so others depend
//! on what it promises, not on how it does it:
//!
//! ```
//! # use quireink_sdk::services::Services;
//! # use quireink_sdk::Id;
//! # extern crate alloc;
//! # use alloc::boxed::Box;
//! /// In the publishing plugin's crate.
//! pub trait Weather {
//!     fn today(&self) -> &str;
//! }
//! pub const WEATHER: Id = Id::new_static("weather.service");
//!
//! struct Sunny;
//! impl Weather for Sunny {
//!     fn today(&self) -> &str {
//!         "sunny"
//!     }
//! }
//!
//! // `register` of the publishing plugin:
//! // r.add_service(WEATHER, Box::new(Box::new(Sunny) as Box<dyn Weather>))?;
//! # let mut services = Services::new();
//! # services.add(WEATHER, Box::new(Box::new(Sunny) as Box<dyn Weather>));
//!
//! // Any plugin, with an `Env`: `env.services.get::<…>(…)`.
//! let weather = services.get::<Box<dyn Weather>>(WEATHER.as_str());
//! assert_eq!(weather.map(|w| w.today()), Some("sunny"));
//! ```
//!
//! A service is shared by all and lives as long as QuireInk runs; one
//! that changes keeps its state in a `Cell` or `RefCell`. One that is
//! missing (the plugin is switched off) is `None`: plugins that use
//! another's service do without it.
//!
//! **Points to contribute to.** A plugin names a point (an id in its
//! namespace) and documents what a contribution there is, e.g. a line
//! of text in a format of its own; any plugin adds to it with
//! [`Registrar::contribute`](crate::Registrar::contribute), and the
//! plugin reads them all with [`Services::contributions`]. The kernel's
//! own points (status items, widgets, text filters …) have registrar
//! methods of their own; these are for points that plugins define.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::any::Any;
use core::fmt;

use crate::Id;

/// Every service and contribution; see the [module documentation](self).
#[derive(Default)]
pub struct Services {
    services: Vec<(Id, Box<dyn Any>)>,
    /// Point, plugin, contribution.
    contributions: Vec<(Id, Id, Cow<'static, str>)>,
}

impl Services {
    pub fn new() -> Self {
        Self::default()
    }

    /// Publishes `service` as `id` (kernel; plugins use
    /// [`Registrar::add_service`](crate::Registrar::add_service)).
    pub fn add(&mut self, id: Id, service: Box<dyn Any>) {
        self.services.push((id, service));
    }

    /// Adds `plugin`'s `contribution` to `point` (kernel; plugins use
    /// [`Registrar::contribute`](crate::Registrar::contribute)).
    pub fn contribute(&mut self, point: Id, plugin: Id, contribution: Cow<'static, str>) {
        self.contributions.push((point, plugin, contribution));
    }

    pub fn shrink_to_fit(&mut self) {
        self.services.shrink_to_fit();
        self.contributions.shrink_to_fit();
    }

    /// Service `id`, if it is published and of type `T`.
    pub fn get<T: Any>(&self, id: &str) -> Option<&T> {
        self.services
            .iter()
            .find(|(s, _)| s.as_str() == id)
            .and_then(|(_, service)| service.downcast_ref::<T>())
    }

    /// Whether service `id` is published.
    pub fn has(&self, id: &str) -> bool {
        self.services.iter().any(|(s, _)| s.as_str() == id)
    }

    /// The contributions to `point`, with the plugin each comes from, in
    /// the order the plugins registered.
    pub fn contributions<'a>(&'a self, point: &'a str) -> impl Iterator<Item = (&'a Id, &'a str)> {
        self.contributions
            .iter()
            .filter(move |(p, _, _)| p.as_str() == point)
            .map(|(_, plugin, text)| (plugin, text.as_ref()))
    }
}

impl fmt::Debug for Services {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Services")
            .field(
                "services",
                &self.services.iter().map(|(id, _)| id).collect::<Vec<_>>(),
            )
            .field("contributions", &self.contributions.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn services_by_id_and_type() {
        let mut services = Services::new();
        services.add(Id::new_static("a.count"), Box::new(3u32));
        assert_eq!(services.get::<u32>("a.count"), Some(&3));
        assert_eq!(services.get::<i64>("a.count"), None, "another type");
        assert_eq!(services.get::<u32>("b.count"), None);
        assert!(services.has("a.count"));

        let point = Id::new_static("a.shapes");
        services.contribute(point.clone(), Id::new_static("b"), Cow::Borrowed("circle"));
        services.contribute(
            Id::new_static("a.other"),
            Id::new_static("b"),
            Cow::Borrowed("x"),
        );
        services.contribute(point, Id::new_static("c"), Cow::Borrowed("square"));
        let shapes: Vec<(&str, &str)> = services
            .contributions("a.shapes")
            .map(|(plugin, text)| (plugin.as_str(), text))
            .collect();
        assert_eq!(shapes, [("b", "circle"), ("c", "square")]);
    }
}
