//! Hand gestures through the webcam, behind the `hands` feature: OpenCV
//! reads the camera and runs MediaPipe's palm and hand models through its
//! `dnn` module ([`camera`], with their arithmetic in [`model`]); what
//! the hands do to the board is [`gesture`]'s.

pub mod camera;
pub mod gesture;
pub mod model;
