//! Rendering the desktop off-screen: one virtual monitor (Smithay `Output`)
//! per screen of the layout, drawn with pixman into its own image and copied
//! into the shared framebuffer viewers see.

use farsight_codec::Framebuffer;
use farsight_rfb::{Rect, Screen};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::surface::{
    WaylandSurfaceRenderElement, render_elements_from_surface_tree,
};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::pixman::{PixmanRenderer, PixmanTarget};
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::backend::renderer::{Bind, ExportMem, Offscreen};
use smithay::desktop::{Space, Window};
use smithay::input::pointer::{CursorImageStatus, CursorImageSurfaceData};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::pixman::Image;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{Buffer as BufferCoords, Physical, Rectangle, Size, Transform};
use smithay::wayland::compositor::with_states;

use crate::state::State;
use crate::{CursorShape, Frame};

/// The desktop background (a calm dark blue-grey).
const BACKGROUND: [f32; 4] = [0.13, 0.15, 0.19, 1.0];
/// Largest cursor image sent to viewers.
const MAX_CURSOR_SIDE: i32 = 256;
/// Most damage rectangles kept for the agent before sending everything.
const MAX_UNSENT_RECTS: usize = 64;
/// Refresh rate advertised for the virtual monitors (mHz).
const REFRESH_MHZ: i32 = 60_000;
/// The "others are watching" frame around each screen (privacy condition
/// of the M7 council): its width and colour (Farsight's "live" red).
const INDICATOR_WIDTH: i32 = 4;
const INDICATOR_COLOUR: [f32; 4] = [0.86, 0.15, 0.15, 1.0];

type Element = WaylandSurfaceRenderElement<PixmanRenderer>;

/// One virtual monitor.
struct Head {
    id: u32,
    output: Output,
    global: GlobalId,
    image: Image<'static, 'static>,
    tracker: OutputDamageTracker,
    /// Where it sits in the framebuffer.
    rect: Rect,
    /// Whether `image` holds the previous frame (so only damage is redrawn).
    drawn_before: bool,
    /// Stable ids for the four edges of the indicator frame.
    indicator_ids: [Id; 4],
}

/// All monitors plus the combined framebuffer.
pub(crate) struct Screens {
    renderer: PixmanRenderer,
    heads: Vec<Head>,
    framebuffer: Framebuffer,
    /// Damage not yet delivered to the agent.
    unsent: Unsent,
    /// Show the "others are watching" frame.
    indicator: bool,
    /// Bumped by every reconfigure; stamped on frames.
    generation: u64,
}

/// What the agent hasn't been sent yet.
#[derive(Debug, Clone)]
enum Unsent {
    Nothing,
    Areas(Vec<Rect>),
    Everything,
}

impl Screens {
    pub fn new(
        dh: &DisplayHandle,
        space: &mut Space<Window>,
        width: u16,
        height: u16,
        layout: &[Screen],
    ) -> Result<Self, String> {
        let renderer = PixmanRenderer::new().map_err(|e| format!("pixman: {e}"))?;
        let mut screens = Self {
            renderer,
            heads: Vec::new(),
            framebuffer: Framebuffer::new(width, height),
            unsent: Unsent::Everything,
            indicator: false,
            generation: 0,
        };
        screens.reconfigure(dh, space, width, height, layout)?;
        Ok(screens)
    }

    /// The current layout generation.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn size(&self) -> (u16, u16) {
        (self.framebuffer.width(), self.framebuffer.height())
    }

    /// Applies a new (already validated) layout: monitors are kept, moved,
    /// resized, added or removed to match, and everything is redrawn.
    pub fn reconfigure(
        &mut self,
        dh: &DisplayHandle,
        space: &mut Space<Window>,
        width: u16,
        height: u16,
        layout: &[Screen],
    ) -> Result<(), String> {
        // Allocate every image first: if one fails, nothing has changed yet.
        let mut images = Vec::with_capacity(layout.len());
        for screen in layout {
            let size = (i32::from(screen.width), i32::from(screen.height));
            images.push(
                self.renderer
                    .create_buffer(Fourcc::Xrgb8888, Size::<i32, BufferCoords>::from(size))
                    .map_err(|e| format!("screen image: {e}"))?,
            );
        }
        let mut kept = Vec::with_capacity(layout.len());
        for (screen, image) in layout.iter().zip(images) {
            let size = (i32::from(screen.width), i32::from(screen.height));
            let mode = Mode {
                size: size.into(),
                refresh: REFRESH_MHZ,
            };
            let location = (i32::from(screen.x), i32::from(screen.y));
            let existing = self.heads.iter().position(|h| h.id == screen.id);
            let (output, global) = if let Some(index) = existing {
                let head = self.heads.swap_remove(index);
                (head.output, head.global)
            } else {
                let output = Output::new(
                    format!("FARSIGHT-{}", screen.id),
                    PhysicalProperties {
                        size: (0, 0).into(),
                        subpixel: Subpixel::Unknown,
                        make: "Farsight".into(),
                        model: "Headless".into(),
                    },
                );
                let global = output.create_global::<State>(dh);
                (output, global)
            };
            output.change_current_state(
                Some(mode),
                Some(Transform::Normal),
                None,
                Some(location.into()),
            );
            output.set_preferred(mode);
            space.map_output(&output, location);
            kept.push(Head {
                id: screen.id,
                tracker: OutputDamageTracker::from_output(&output),
                output,
                global,
                image,
                rect: Rect {
                    x: screen.x,
                    y: screen.y,
                    width: screen.width,
                    height: screen.height,
                },
                drawn_before: false,
                indicator_ids: [Id::new(), Id::new(), Id::new(), Id::new()],
            });
        }
        for removed in std::mem::replace(&mut self.heads, kept) {
            space.unmap_output(&removed.output);
            dh.remove_global::<State>(removed.global);
        }
        self.framebuffer = Framebuffer::new(width, height);
        self.unsent = Unsent::Everything;
        self.generation += 1;
        Ok(())
    }

    /// Shows or hides the frame that tells people in the desktop that more
    /// than one viewer is connected.
    pub fn set_indicator(&mut self, shown: bool) {
        self.indicator = shown;
    }

    /// Whether a frame is waiting to be delivered.
    pub fn has_unsent_frame(&self) -> bool {
        !matches!(self.unsent, Unsent::Nothing)
    }

    /// The agent took the last frame.
    pub fn frame_sent(&mut self) {
        self.unsent = Unsent::Nothing;
    }

    /// Redraws what changed and returns the frame to deliver, if there is
    /// anything new (or still undelivered).
    pub fn render(&mut self, space: &Space<Window>) -> Option<Frame> {
        for head in &mut self.heads {
            let damage = render_head(
                &mut self.renderer,
                &mut self.framebuffer,
                space,
                head,
                self.indicator,
            );
            match (&mut self.unsent, damage) {
                (Unsent::Nothing, Some(rects)) => self.unsent = Unsent::Areas(rects),
                (Unsent::Areas(all), Some(rects)) => {
                    all.extend(rects);
                    // While the agent is behind, don't let the list grow forever.
                    if all.len() > MAX_UNSENT_RECTS {
                        self.unsent = Unsent::Everything;
                    }
                }
                (_, None) | (Unsent::Everything, Some(_)) => {}
            }
        }
        let damage = match &self.unsent {
            Unsent::Nothing => return None,
            Unsent::Areas(rects) => Some(rects.clone()),
            Unsent::Everything => None,
        };
        Some(Frame {
            framebuffer: self.framebuffer.clone(),
            damage,
            generation: self.generation,
        })
    }

    /// Draws the current cursor for viewers (`None` = hidden).
    pub fn cursor_shape(&mut self, status: &CursorImageStatus) -> Option<CursorShape> {
        match status {
            CursorImageStatus::Hidden => None,
            CursorImageStatus::Named(_) => Some(default_arrow()),
            CursorImageStatus::Surface(surface) => {
                let hotspot = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<CursorImageSurfaceData>()
                        .and_then(|data| data.lock().ok().map(|d| d.hotspot))
                })
                .unwrap_or_default();
                let size = with_renderer_surface_state(surface, |s| s.buffer_size()).flatten()?;
                let (w, h) = (size.w.min(MAX_CURSOR_SIDE), size.h.min(MAX_CURSOR_SIDE));
                if w <= 0 || h <= 0 {
                    return None;
                }
                let mut image = self
                    .renderer
                    .create_buffer(Fourcc::Abgr8888, Size::<i32, BufferCoords>::from((w, h)))
                    .ok()?;
                let elements: Vec<Element> = render_elements_from_surface_tree(
                    &mut self.renderer,
                    surface,
                    (0, 0),
                    1.0,
                    1.0,
                    Kind::Cursor,
                );
                let mut tracker = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
                let mut target = self.renderer.bind(&mut image).ok()?;
                tracker
                    .render_output(&mut self.renderer, &mut target, 0, &elements, [0.0; 4])
                    .ok()?;
                let premultiplied = read(
                    &mut self.renderer,
                    &target,
                    Rectangle::from_size((w, h).into()),
                    Fourcc::Abgr8888,
                )?;
                Some(CursorShape {
                    width: u16::try_from(w).ok()?,
                    height: u16::try_from(h).ok()?,
                    hotspot: (
                        u16::try_from(hotspot.x.clamp(0, w - 1)).ok()?,
                        u16::try_from(hotspot.y.clamp(0, h - 1)).ok()?,
                    ),
                    rgba: unpremultiply(premultiplied),
                })
            }
        }
    }
}

/// Renders one monitor; returns the damaged areas in framebuffer
/// coordinates, already copied into `framebuffer`.
fn render_head(
    renderer: &mut PixmanRenderer,
    framebuffer: &mut Framebuffer,
    space: &Space<Window>,
    head: &mut Head,
    indicator: bool,
) -> Option<Vec<Rect>> {
    let age = usize::from(head.drawn_before);
    let frame = if indicator {
        indicator_frame(head)
    } else {
        Vec::new()
    };
    let mut target = renderer.bind(&mut head.image).ok()?;
    let damage = smithay::desktop::space::render_output::<_, SolidColorRenderElement, _, _>(
        &head.output,
        renderer,
        &mut target,
        1.0,
        age,
        [space],
        &frame,
        &mut head.tracker,
        BACKGROUND,
    )
    .ok()?
    .damage
    .cloned()?;
    head.drawn_before = true;
    let mut rects = Vec::with_capacity(damage.len());
    for area in damage {
        let Some(rect) = clip(area, head.rect) else {
            continue;
        };
        let region = Rectangle::new(
            (
                i32::from(rect.x - head.rect.x),
                i32::from(rect.y - head.rect.y),
            )
                .into(),
            (i32::from(rect.width), i32::from(rect.height)).into(),
        );
        let Some(bytes) = read(renderer, &target, region, Fourcc::Xrgb8888) else {
            continue;
        };
        let written = framebuffer.write(rect, |i| {
            let at = i * 4;
            match bytes.get(at..at + 4) {
                Some(&[b, g, r, _]) => u32::from_le_bytes([b, g, r, 0]),
                _ => 0,
            }
        });
        if written.is_ok() {
            rects.push(rect);
        }
    }
    (!rects.is_empty()).then_some(rects)
}

/// The four edges of the indicator frame, in monitor coordinates.
fn indicator_frame(head: &Head) -> Vec<SolidColorRenderElement> {
    let (w, h) = (i32::from(head.rect.width), i32::from(head.rect.height));
    let edge = INDICATOR_WIDTH.min(w).min(h);
    let edges = [
        Rectangle::new((0, 0).into(), (w, edge).into()),
        Rectangle::new((0, h - edge).into(), (w, edge).into()),
        Rectangle::new((0, 0).into(), (edge, h).into()),
        Rectangle::new((w - edge, 0).into(), (edge, h).into()),
    ];
    head.indicator_ids
        .iter()
        .zip(edges)
        .map(|(id, geometry)| {
            SolidColorRenderElement::new(
                id.clone(),
                geometry,
                0,
                INDICATOR_COLOUR,
                Kind::Unspecified,
            )
        })
        .collect()
}

/// Reads `region` of the bound image as tightly packed 4-byte pixels.
fn read(
    renderer: &mut PixmanRenderer,
    target: &PixmanTarget<'_>,
    region: Rectangle<i32, BufferCoords>,
    format: Fourcc,
) -> Option<Vec<u8>> {
    let mapping = renderer.copy_framebuffer(target, region, format).ok()?;
    renderer.map_texture(&mapping).ok().map(<[u8]>::to_vec)
}

/// Converts a monitor-local damage rectangle into framebuffer coordinates,
/// clipped to the monitor.
fn clip(area: Rectangle<i32, Physical>, head: Rect) -> Option<Rect> {
    let x0 = area.loc.x.max(0);
    let y0 = area.loc.y.max(0);
    let x1 = (area.loc.x + area.size.w).min(i32::from(head.width));
    let y1 = (area.loc.y + area.size.h).min(i32::from(head.height));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(Rect {
        x: head.x + u16::try_from(x0).ok()?,
        y: head.y + u16::try_from(y0).ok()?,
        width: u16::try_from(x1 - x0).ok()?,
        height: u16::try_from(y1 - y0).ok()?,
    })
}

/// pixman stores premultiplied alpha; viewers want straight RGBA.
fn unpremultiply(mut rgba: Vec<u8>) -> Vec<u8> {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let [r, g, b, a] = pixel;
        let alpha = u16::from(*a);
        if alpha != 0 && alpha != 255 {
            for channel in [r, g, b] {
                *channel =
                    u8::try_from((u16::from(*channel) * 255 + alpha / 2) / alpha).unwrap_or(255);
            }
        }
    }
    rgba
}

/// A plain arrow, used when an app asks for a named cursor (no cursor theme
/// is loaded in the headless compositor).
fn default_arrow() -> CursorShape {
    const ROWS: [&str; 17] = [
        "X          ",
        "XX         ",
        "X.X        ",
        "X..X       ",
        "X...X      ",
        "X....X     ",
        "X.....X    ",
        "X......X   ",
        "X.......X  ",
        "X........X ",
        "X.....XXXXX",
        "X..X..X    ",
        "X.X X..X   ",
        "XX  X..X   ",
        "X    X..X  ",
        "     X..X  ",
        "      XX   ",
    ];
    let width = ROWS.first().map_or(0, |row| row.len());
    let rgba = ROWS
        .iter()
        .flat_map(|row| row.bytes())
        .flat_map(|cell| match cell {
            b'X' => [0, 0, 0, 255],
            b'.' => [255, 255, 255, 255],
            _ => [0, 0, 0, 0],
        })
        .collect();
    CursorShape {
        width: u16::try_from(width).unwrap_or(0),
        height: u16::try_from(ROWS.len()).unwrap_or(0),
        hotspot: (0, 0),
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damage_is_clipped_and_offset_to_the_monitor() {
        let head = Rect {
            x: 100,
            y: 0,
            width: 50,
            height: 40,
        };
        let area = Rectangle::new((-5, 30).into(), (20, 20).into());
        assert_eq!(
            clip(area, head),
            Some(Rect {
                x: 100,
                y: 30,
                width: 15,
                height: 10
            })
        );
        assert_eq!(
            clip(Rectangle::new((60, 0).into(), (5, 5).into()), head),
            None
        );
    }

    #[test]
    fn premultiplied_pixels_are_restored() {
        assert_eq!(
            unpremultiply(vec![64, 32, 0, 128, 9, 9, 9, 0]),
            vec![128, 64, 0, 128, 9, 9, 9, 0]
        );
    }

    #[test]
    fn the_default_arrow_is_well_formed() {
        let arrow = default_arrow();
        assert_eq!(
            arrow.rgba.len(),
            usize::from(arrow.width) * usize::from(arrow.height) * 4
        );
    }
}
