//! PDF books through the system's renderer (`android.graphics.pdf.PdfRenderer`,
//! Android 5 and later).
//!
//! Each page is a section of one picture, rendered to fit the screen when
//! it is first shown and kept in the gray cache, like the pages of an XTC
//! book. Only the pages around the one being read are kept, so long books
//! do not fill the storage. The renderer knows no text: PDF books have no
//! search, no reading aloud and no table of contents (yet).

use std::cell::RefCell;

use android_activity::AndroidApp;
use jni::JNIEnv;
use jni::objects::{GlobalRef, JObject, JValue};
use quireink_sdk::document::{
    Align, Block, BlockKind, Document, DocumentError, ImageRef, Locator, Metadata, OffsetMap,
};
use quireink_sdk::image::{GrayFile, GrayWriter, ImageSink};
use quireink_sdk::{Env, Storage};

use crate::access::with_activity;

/// `ParcelFileDescriptor.MODE_READ_ONLY`.
const MODE_READ_ONLY: i32 = 0x1000_0000;
/// `PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY`.
const RENDER_FOR_DISPLAY: i32 = 1;
/// Rows of a page copied from the renderer's bitmap at a time.
const BAND: i32 = 64;
/// Rendered pages kept in the cache.
const KEPT_PAGES: usize = 12;

fn failed(what: &'static str) -> DocumentError {
    DocumentError::Malformed(what.into())
}

/// FNV-1a, for a cache folder per book.
fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// An open PDF book.
pub struct PdfDocument {
    app: AndroidApp,
    /// The renderer and the file it reads.
    renderer: GlobalRef,
    file: GlobalRef,
    pages: u32,
    metadata: Metadata,
    /// The largest picture a page is rendered to (the screen).
    fit: (u32, u32),
    /// Cache folder of the rendered pages.
    cache: String,
    /// Pages in the cache, the most recently shown last.
    cached: RefCell<Vec<u32>>,
}

impl PdfDocument {
    /// Opens the book at `path`, rendering its pages to fit `fit`.
    pub fn open(
        app: &AndroidApp,
        storage: &dyn Storage,
        path: &str,
        fit: (u32, u32),
    ) -> Result<Self, DocumentError> {
        let mut real = None;
        storage.local_path(path, &mut |p| real = Some(p.to_string()));
        let real = real.ok_or(failed("not a local file"))?;
        let (renderer, file, pages) = with_activity(app, |env, _| {
            let name = env.new_string(&real)?;
            let java_file =
                env.new_object("java/io/File", "(Ljava/lang/String;)V", &[(&name).into()])?;
            let file = env
                .call_static_method(
                    "android/os/ParcelFileDescriptor",
                    "open",
                    "(Ljava/io/File;I)Landroid/os/ParcelFileDescriptor;",
                    &[(&java_file).into(), JValue::Int(MODE_READ_ONLY)],
                )?
                .l()?;
            // Protected or broken files throw here.
            let renderer = env.new_object(
                "android/graphics/pdf/PdfRenderer",
                "(Landroid/os/ParcelFileDescriptor;)V",
                &[(&file).into()],
            );
            let renderer = match renderer {
                Ok(renderer) => renderer,
                Err(e) => {
                    let _ = env.exception_clear();
                    let _ = env.call_method(&file, "close", "()V", &[]);
                    return Err(e);
                }
            };
            let pages = env
                .call_method(&renderer, "getPageCount", "()I", &[])?
                .i()?;
            Ok((
                env.new_global_ref(renderer)?,
                env.new_global_ref(file)?,
                pages,
            ))
        })
        .ok_or(failed("PDF"))?;
        let name = path.rsplit('/').next().unwrap_or(path);
        let metadata = Metadata {
            title: Some(name.rsplit_once('.').map_or(name, |(s, _)| s).to_string()),
            ..Metadata::default()
        };
        let cache = format!(
            "{}/{:016x}",
            Env::plugin_dir(crate::plugin::PLUGIN.as_str()),
            fnv(path.as_bytes())
        );
        // Pages rendered from another file or for another screen go.
        let stamp = format!("{} {pages} {}x{}\n", storage.size(path)?, fit.0, fit.1);
        let stamp_path = format!("{cache}/source");
        if quireink_sdk::read_file(storage, &stamp_path, 64).ok()
            != Some(stamp.clone().into_bytes())
        {
            let _ = storage.remove_dir_all(&cache);
            let _ = storage.create_dir(&cache);
            storage.write(&stamp_path, stamp.as_bytes())?;
        }
        let mut cached = Vec::new();
        let _ = storage.list(&cache, &mut |entry| {
            if let Some(page) = entry
                .name
                .strip_prefix('p')
                .and_then(|n| n.strip_suffix(".gray"))
                .and_then(|n| n.parse().ok())
            {
                cached.push(page);
            }
        });
        Ok(Self {
            app: app.clone(),
            renderer,
            file,
            pages: u32::try_from(pages).unwrap_or(0).max(1),
            metadata,
            fit,
            cache,
            cached: RefCell::new(cached),
        })
    }

    fn page_path(&self, page: u32) -> String {
        format!("{}/p{page}.gray", self.cache)
    }

    /// Page `page` as a gray picture in the cache, rendered if needed.
    fn page(&self, storage: &dyn Storage, page: u32) -> Result<ImageRef, DocumentError> {
        let path = self.page_path(page);
        if GrayFile::open(storage, &path).is_none() {
            let mut writer = GrayWriter::new(storage, &path);
            let rendered = with_activity(&self.app, |env, _| self.render(env, page, &mut writer))
                .ok_or(failed("page"))
                .and_then(|r| r.map_err(|_| failed("page")))
                .and_then(|()| writer.finish().map_err(|_| failed("page")));
            if let Err(e) = rendered {
                let _ = storage.remove(&path);
                return Err(e);
            }
            self.keep(storage, page);
        }
        let file = GrayFile::open(storage, &path).ok_or(failed("page"))?;
        Ok(ImageRef {
            path: file.path,
            width: file.width,
            height: file.height,
        })
    }

    /// Notes `page` as rendered; drops the pages farthest from it when
    /// there are too many.
    fn keep(&self, storage: &dyn Storage, page: u32) {
        let mut cached = self.cached.borrow_mut();
        cached.retain(|&p| p != page);
        cached.push(page);
        while cached.len() > KEPT_PAGES {
            let (at, _) = cached
                .iter()
                .enumerate()
                .max_by_key(|(_, p)| p.abs_diff(page))
                .unwrap_or((0, &0));
            let gone = cached.remove(at);
            let _ = storage.remove(&self.page_path(gone));
        }
    }

    /// Renders `page` into `sink`, scaled to fit the screen, on white.
    fn render(
        &self,
        env: &mut JNIEnv<'_>,
        page: u32,
        sink: &mut dyn ImageSink,
    ) -> jni::errors::Result<Result<(), ()>> {
        let open = env
            .call_method(
                self.renderer.as_obj(),
                "openPage",
                "(I)Landroid/graphics/pdf/PdfRenderer$Page;",
                &[JValue::Int(page as i32)],
            )?
            .l()?;
        let result = self.render_open(env, &open, sink);
        let _ = env.call_method(&open, "close", "()V", &[]);
        result
    }

    fn render_open(
        &self,
        env: &mut JNIEnv<'_>,
        page: &JObject<'_>,
        sink: &mut dyn ImageSink,
    ) -> jni::errors::Result<Result<(), ()>> {
        let width = env.call_method(page, "getWidth", "()I", &[])?.i()?.max(1);
        let height = env.call_method(page, "getHeight", "()I", &[])?.i()?.max(1);
        // Points to pixels: as large as fits the screen.
        let scale = (self.fit.0 as f32 / width as f32).min(self.fit.1 as f32 / height as f32);
        let w = ((width as f32 * scale) as i32).max(1);
        let h = ((height as f32 * scale) as i32).max(1);
        let config = env
            .get_static_field(
                "android/graphics/Bitmap$Config",
                "ARGB_8888",
                "Landroid/graphics/Bitmap$Config;",
            )?
            .l()?;
        let bitmap = env
            .call_static_method(
                "android/graphics/Bitmap",
                "createBitmap",
                "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
                &[JValue::Int(w), JValue::Int(h), (&config).into()],
            )?
            .l()?;
        // Pages are transparent where nothing is printed.
        env.call_method(&bitmap, "eraseColor", "(I)V", &[JValue::Int(-1)])?;
        let null = JObject::null();
        env.call_method(
            page,
            "render",
            "(Landroid/graphics/Bitmap;Landroid/graphics/Rect;Landroid/graphics/Matrix;I)V",
            &[
                (&bitmap).into(),
                (&null).into(),
                (&null).into(),
                JValue::Int(RENDER_FOR_DISPLAY),
            ],
        )?;
        let result = self.copy_rows(env, &bitmap, w, h, sink);
        let _ = env.call_method(&bitmap, "recycle", "()V", &[]);
        result
    }

    /// Copies the bitmap into `sink` as gray, a band of rows at a time.
    fn copy_rows(
        &self,
        env: &mut JNIEnv<'_>,
        bitmap: &JObject<'_>,
        w: i32,
        h: i32,
        sink: &mut dyn ImageSink,
    ) -> jni::errors::Result<Result<(), ()>> {
        if sink.start(w as u32, h as u32).is_err() {
            return Ok(Err(()));
        }
        let pixels = env.new_int_array(w * BAND)?;
        let mut band = vec![0i32; (w * BAND) as usize];
        let mut row = vec![0u8; w as usize];
        let mut y = 0;
        while y < h {
            let rows = BAND.min(h - y);
            env.call_method(
                bitmap,
                "getPixels",
                "([IIIIIII)V",
                &[
                    (&pixels).into(),
                    JValue::Int(0),
                    JValue::Int(w),
                    JValue::Int(0),
                    JValue::Int(y),
                    JValue::Int(w),
                    JValue::Int(rows),
                ],
            )?;
            env.get_int_array_region(&pixels, 0, &mut band[..(w * rows) as usize])?;
            for r in 0..rows as usize {
                let line = &band[r * w as usize..(r + 1) * w as usize];
                for (out, &argb) in row.iter_mut().zip(line) {
                    let (red, green, blue) = ((argb >> 16) & 0xff, (argb >> 8) & 0xff, argb & 0xff);
                    *out = ((red * 77 + green * 150 + blue * 29) >> 8) as u8;
                }
                if sink.row(&row).is_err() {
                    return Ok(Err(()));
                }
            }
            y += rows;
        }
        Ok(Ok(()))
    }
}

impl Drop for PdfDocument {
    fn drop(&mut self) {
        with_activity(&self.app, |env, _| {
            env.call_method(self.renderer.as_obj(), "close", "()V", &[])?;
            env.call_method(self.file.as_obj(), "close", "()V", &[])?;
            Ok(())
        });
    }
}

impl Document for PdfDocument {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn whole_pages(&self) -> bool {
        true
    }

    fn sections(&self) -> u32 {
        self.pages
    }

    fn section_len(&self, _: u32) -> u32 {
        1
    }

    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
        if at.section >= self.pages || at.offset >= 1 {
            return Ok(None);
        }
        let image = self.page(storage, at.section).ok();
        let (kind, text) = match &image {
            Some(_) => (BlockKind::Image, String::new()),
            None => (BlockKind::Paragraph, format!("{}", at.section + 1)),
        };
        Ok(Some(Block {
            kind,
            continues: false,
            start: 0,
            end: 1,
            text,
            map: OffsetMap::IDENTITY,
            runs: Vec::new(),
            align: Align::Center,
            indent: None,
            space: None,
            size: None,
            image,
        }))
    }

    fn cover(&self, storage: &dyn Storage) -> Result<Option<ImageRef>, DocumentError> {
        Ok(self.page(storage, 0).ok())
    }
}
