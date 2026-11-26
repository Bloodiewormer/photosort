//! Local vision models (SigLIP 2 image/text embeddings and YuNet face detection).
//! Not yet wired into the app or the catalog.

pub mod manifest;
pub mod session;
pub mod siglip2;
pub mod synth;
pub mod yunet;

pub use ocr::engine::{Provider, init_runtime};
pub use session::SessionOptions;
pub use siglip2::{SigLip2, SigLip2Text, SigLip2Vision, cosine_similarity, l2_normalize};
pub use yunet::{Face, YuNet, YuNetConfig};
