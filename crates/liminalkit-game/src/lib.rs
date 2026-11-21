//! liminalkit-game: an interface over a rendered scene. **EXPERIMENTAL** —
//! the API is new, the tuning is judged by eye on one desktop, and the
//! reference shader has been validated as WGSL but not run on a GPU from this
//! workspace, which has none.
//!
//! The rest of liminalkit draws one screen into one buffer and costs nothing
//! while nothing changes. A game is the opposite case: its scene changes
//! every frame, and its interface moves over that scene — fading in, sliding,
//! lying on a planet that turns. This crate is what that needs, built on the
//! same controls and held to the same rule: state lives in structs the
//! application owns, and time is told, never read.
//!
//! # The idea
//!
//! Paint rarely, place every frame. Each piece of interface is a
//! `layer::Layer`: a `surface::Surface` the ordinary immediate-mode `Ui`
//! paints into when its content changes, a `sheet::Sheet` that says where
//! that picture goes this frame, and [`effect::Effects`] applied on the way.
//! Everything that moves is done to the picture, not by repainting it.
//!
//! One frame is described once, as a `frame::GameFrame`, and drawn either by
//! `composite::composite` on the CPU or by the host's own GPU from
//! `GameFrame::gpu` and the reference shader in [`gpu`]. This crate has no
//! GPU dependency: the GPU path is data and shader text.
//!
//! # A frame
//!
//! 1. As pointer events arrive, give them to `input::Router::pointer`. It
//!    reports whether the interface took each one; the rest are the world's.
//! 2. Advance what the application animates: [`anim`] tweens and springs, a
//!    [`notify::NoticeQueue`], [`anchor::place`] for popovers that follow
//!    points of the world.
//! 3. Paint each layer whose `Surface::needs_paint`.
//! 4. `Layer::prepare` each layer with the host's time and, for layers lying
//!    on something, a `sheet::Scene`: the surface and the camera, described
//!    through [`world::Surface3`] and [`view::View`].
//! 5. Push the layers into a `GameFrame` and draw it.
//!
//! # Without the rest of liminalkit
//!
//! The layer, surface, sheet, frame, compositor and router modules are behind
//! the `surfaces` feature, which is on by default. With
//! `default-features = false` the crate needs only `std` and keeps [`anim`],
//! [`effect`] (parameters and their scalar specification), [`notify`],
//! [`anchor`], [`gpu`], and the geometry in [`math`], [`view`] and
//! [`world`]: the parts a game with its own interface renderer can use.

pub mod anchor;
pub mod anim;
pub mod effect;
pub mod geom;
pub mod gpu;
pub mod math;
pub mod notify;
pub mod view;
pub mod world;

#[cfg(feature = "surfaces")]
pub mod composite;
#[cfg(feature = "surfaces")]
pub mod frame;
#[cfg(feature = "surfaces")]
pub mod input;
#[cfg(feature = "surfaces")]
pub mod layer;
#[cfg(feature = "surfaces")]
pub mod sheet;
#[cfg(feature = "surfaces")]
pub mod surface;
#[cfg(feature = "surfaces")]
pub mod warp;
