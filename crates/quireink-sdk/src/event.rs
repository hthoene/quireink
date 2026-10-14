use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::command::Command;
use crate::env::Env;
use crate::params::Params;

/// Reacts to an event by emitting commands.
///
/// Events are notifications: the kernel and plugins emit them with
/// [`Command::Emit`], and every subscriber is called in registration order.
/// Any plugin can define its own events (namespaced ids such as
/// `reader.page-turned`), which other plugins may subscribe to.
pub type EventHandler = Box<dyn Fn(&Env<'_>, &Params, &mut Vec<Command>)>;
