//! Status items "Time left in chapter" and "Time left in book": the pages
//! still to read at one's reading speed, as recorded by the statistics.

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::rc::Rc;
use core::cell::Cell;

use quireink_sdk::builtin::{events, params};
use quireink_sdk::status::{StatusItemSpec, StatusValue, context};
use quireink_sdk::{Arg, Env, Id, Params, RegisterError, Registrar};

use crate::{fmt, store};

pub(crate) const CHAPTER: Id = Id::new_static("stats.status.left-chapter");
pub(crate) const BOOK: Id = Id::new_static("stats.status.left-book");
/// Days the reading speed is taken from.
const DAYS: i32 = 30;
/// Pages read before there is a speed to go by.
const MIN_PAGES: u32 = 10;
/// Pages turned after which the speed is worked out again.
const RECOUNT: u32 = 50;

/// Seconds a page, from the reading of the last [`DAYS`] days, if enough
/// was read.
fn speed(env: &Env<'_>) -> Option<f32> {
    let today = fmt::today(env)?;
    let (mut seconds, mut pages) = (0u64, 0u32);
    for day in store::days(env.storage, |unix| fmt::day_of(env, unix)) {
        if day.day > today - DAYS {
            seconds += u64::from(day.seconds);
            pages += day.stays;
        }
    }
    (pages >= MIN_PAGES).then(|| seconds as f32 / pages as f32)
}

/// Pages left in the book (`chapter` false) or in the chapter, from the
/// reader's context: the pages counted, the progress and where the
/// chapters begin.
fn pages_left(context: &Params, chapter: bool) -> Option<f32> {
    let pages: f32 = context.get(params::PAGES)?.parse().ok()?;
    let progress: f32 = context.get(params::PROGRESS)?.parse().ok()?;
    let at = progress * 10.0;
    let end = if chapter {
        context
            .get(context::CHAPTERS)
            .unwrap_or_default()
            .split(',')
            .filter_map(|mark| mark.trim().parse::<f32>().ok())
            .find(|&mark| mark > at)
            .unwrap_or(1000.0)
    } else {
        1000.0
    };
    Some(((end - at) / 1000.0 * pages).max(0.0))
}

pub(crate) fn register(r: &mut dyn Registrar) -> Result<(), RegisterError> {
    // The speed is worked out once a book is opened (and on the first
    // use), not for every page.
    let speed_known: Rc<Cell<Option<Option<f32>>>> = Rc::new(Cell::new(None));
    let known = speed_known.clone();
    r.subscribe(
        events::BOOK_OPENED,
        Box::new(move |_, _, _| known.set(None)),
    )?;
    // While there is none yet, and now and then while reading, it is
    // worked out again.
    let known = speed_known.clone();
    let turns = Rc::new(Cell::new(0u32));
    r.subscribe(
        events::PAGE_TURNED,
        Box::new(move |_, _, _| {
            turns.set(turns.get().wrapping_add(1));
            if known.get() == Some(None) || turns.get() % RECOUNT == 0 {
                known.set(None);
            }
        }),
    )?;
    for (id, title, key, chapter) in [
        (
            CHAPTER,
            "stats.status.left-chapter",
            "stats.left.chapter",
            true,
        ),
        (BOOK, "stats.status.left-book", "stats.left.book", false),
    ] {
        let known = speed_known.clone();
        r.add_status_item(StatusItemSpec {
            id,
            title: Cow::Borrowed(title),
            value: Box::new(move |env, context| {
                let speed = match known.get() {
                    Some(speed) => speed,
                    None => {
                        let speed = speed(env);
                        known.set(Some(speed));
                        speed
                    }
                }?;
                let seconds = (pages_left(context, chapter)? * speed) as u64;
                let time = fmt::duration(env, seconds.max(60));
                Some(StatusValue::Text(
                    env.i18n.format(key, &[("time", Arg::Str(&time))]),
                ))
            }),
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_left_to_the_next_chapter() {
        let context = Params::new()
            .with(params::PAGES, "200")
            .with(params::PROGRESS, "25.0")
            .with(context::CHAPTERS, "0,100,300,700");
        assert_eq!(pages_left(&context, false), Some(150.0));
        // From 250 to 300 thousandths: 10 of 200 pages.
        assert_eq!(pages_left(&context, true), Some(10.0));
        // In the last chapter: to the end.
        let last = context.with(params::PROGRESS, "80.0");
        assert_eq!(pages_left(&last, true), Some(40.0));
        // Pages not counted yet: nothing to say.
        assert_eq!(
            pages_left(&Params::new().with(params::PROGRESS, "5"), false),
            None
        );
    }
}
