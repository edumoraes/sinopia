//! The camera and the two hand models, on a thread of their own (shell,
//! untested like every other bridge): OpenCV reads the camera and runs
//! the models through its `dnn` module; everything they mean is
//! [`super::model`]'s, and everything the hands do is
//! [`super::gesture`]'s. A reading goes to the event loop through a sink,
//! as the tablet's and the trackpad's do.
//!
//! The frame is mirrored before anything reads it, so a hand moves the
//! way the person sees it move. A hand found is followed from its own
//! points; the palm detector, the heavier model, runs only when no hand
//! is followed, and every few frames while fewer than two are.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use opencv::core::{self, Mat, Scalar, Size, Vector};
use opencv::prelude::*;
use opencv::{dnn, imgproc, videoio};

use super::model::{self, HAND_SIDE, PALM_SIDE, POINTS, Palm, Roi};

/// The models, as OpenCV's model zoo publishes them; what
/// `experiments/hands/fetch-models.sh` puts in the data directory.
pub const PALM_MODEL: &str = "palm_detection_mediapipe_2023feb.onnx";
pub const HAND_MODEL: &str = "handpose_estimation_mediapipe_2023feb.onnx";

/// How sure the landmark model has to be that a hand is in its square:
/// to take one the detector found, and to go on following one.
const FIND: f32 = 0.6;
const KEEP: f32 = 0.5;
/// While a hand is followed, the detector looks for a second only every
/// this many frames.
const SEARCH_EVERY: u64 = 6;
/// The presenter's camera goes to the card this wide, its height in
/// proportion.
const CARD_W: i32 = 480;
/// How often the time a frame takes is told to the log.
const REPORT_EVERY: Duration = Duration::from_secs(10);

/// One hand: its 21 points — x and depth by the frame's width, y by its
/// height, mirrored.
#[derive(Debug, Clone)]
pub struct Hand {
    pub points: [[f32; 3]; POINTS],
}

/// What the camera saw in one frame.
#[derive(Debug, Clone)]
pub struct Reading {
    pub hands: Vec<Hand>,
    /// The frame, in px.
    pub size: (u32, u32),
    /// The frame itself, small, RGBA and mirrored, while the card wants
    /// it.
    pub card: Option<(u32, u32, Vec<u8>)>,
}

/// What the thread tells the loop.
#[derive(Debug, Clone)]
pub enum News {
    /// The models are loaded and the camera is open — or why not, and
    /// then the thread is over.
    Opened(Result<(), String>),
    /// What the camera saw in one frame.
    Read(Reading),
    /// The camera stopped giving frames, and why: the thread is over.
    Lost(String),
}

/// The thread reading the camera, while there is one: dropping it stops
/// the thread and lets go of the camera.
pub struct Tracker {
    stop: Arc<AtomicBool>,
    card: Arc<AtomicBool>,
    /// The camera has opened, or failed to: past that the thread only
    /// ever waits a frame, and dropping the tracker waits for it.
    settled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Tracker {
    /// Starts a thread that loads the models from `models`, opens
    /// `camera` — a device index, or a video file played as one, looped
    /// — and reads it, telling `sink` what happens until it says no.
    /// Answers at once: a camera can take a second or more to open, and
    /// the window does not wait for it. `News::Opened` says when it has,
    /// or why it did not.
    pub fn start(models: &Path, camera: &str, sink: impl Fn(News) -> bool + Send + 'static) -> anyhow::Result<Tracker> {
        let stop = Arc::new(AtomicBool::new(false));
        let card = Arc::new(AtomicBool::new(false));
        let settled = Arc::new(AtomicBool::new(false));
        let (models, camera) = (models.to_owned(), camera.to_owned());
        let (halt, want, done) = (stop.clone(), card.clone(), settled.clone());
        let thread = std::thread::Builder::new().name("hands".into()).spawn(move || {
            let had = Models::load(&models).and_then(|m| Ok((m, open(&camera)?)));
            done.store(true, Ordering::Release);
            let (mut models, mut cap) = match had {
                Ok(had) => had,
                Err(e) => {
                    sink(News::Opened(Err(format!("{e:#}"))));
                    return;
                }
            };
            if halt.load(Ordering::Relaxed) || !sink(News::Opened(Ok(()))) {
                return;
            }
            if let Err(e) = run(&mut models, &mut cap, &camera, &halt, &want, &sink) {
                sink(News::Lost(format!("{e:#}")));
            }
        })?;
        Ok(Tracker {
            stop,
            card,
            settled,
            thread: Some(thread),
        })
    }

    /// Whether the readings carry the frame for the presenter's card.
    pub fn want_card(&self, on: bool) {
        self.card.store(on, Ordering::Relaxed);
    }
}

impl Drop for Tracker {
    /// Stops the thread. Once the camera has opened it stops within a
    /// frame and is waited for, so the camera is let go of before another
    /// tracker asks for it; a camera still opening is not waited on —
    /// the window would hang on it for as long as the driver does — and
    /// the thread ends on its own when the open comes back.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take()
            && self.settled.load(Ordering::Acquire)
        {
            let _ = thread.join();
        }
    }
}

fn open(camera: &str) -> anyhow::Result<videoio::VideoCapture> {
    let mut cap = match camera.parse::<i32>() {
        Ok(index) => videoio::VideoCapture::new(index, videoio::CAP_V4L2)?,
        Err(_) => videoio::VideoCapture::from_file(camera, videoio::CAP_ANY)?,
    };
    anyhow::ensure!(cap.is_opened()?, "camera {camera:?} did not open");
    if camera.parse::<i32>().is_ok() {
        cap.set(videoio::CAP_PROP_FRAME_WIDTH, 640.0)?;
        cap.set(videoio::CAP_PROP_FRAME_HEIGHT, 480.0)?;
        cap.set(videoio::CAP_PROP_FPS, 30.0)?;
        // The latest frame, not a queue of old ones: a hand is where it
        // is now.
        cap.set(videoio::CAP_PROP_BUFFERSIZE, 1.0)?;
    }
    Ok(cap)
}

struct Models {
    palm: dnn::Net,
    hand: dnn::Net,
    palm_out: Vector<String>,
    hand_out: Vector<String>,
    anchors: Vec<model::Point>,
}

impl Models {
    fn load(dir: &Path) -> anyhow::Result<Models> {
        let read = |name: &str| -> anyhow::Result<dnn::Net> {
            let path = dir.join(name);
            anyhow::ensure!(
                path.is_file(),
                "no {name} in {} — experiments/hands/fetch-models.sh puts it there",
                dir.display()
            );
            Ok(dnn::read_net_from_onnx(&path, dnn::ENGINE_AUTO)?)
        };
        let (palm, hand) = (read(PALM_MODEL)?, read(HAND_MODEL)?);
        Ok(Models {
            palm_out: palm.get_unconnected_out_layers_names()?,
            hand_out: hand.get_unconnected_out_layers_names()?,
            palm,
            hand,
            anchors: model::anchors(),
        })
    }

    /// The palms in `rgb`, in its px.
    fn palms(&mut self, rgb: &Mat) -> anyhow::Result<Vec<Palm>> {
        let lb = model::letterbox(rgb.cols(), rgb.rows());
        let mut small = Mat::default();
        imgproc::resize(rgb, &mut small, Size::new(lb.size[0], lb.size[1]), 0.0, 0.0, imgproc::INTER_LINEAR)?;
        let side = PALM_SIDE as i32;
        let mut square = Mat::default();
        core::copy_make_border(
            &small,
            &mut square,
            lb.pad[1],
            side - lb.size[1] - lb.pad[1],
            lb.pad[0],
            side - lb.size[0] - lb.pad[0],
            core::BORDER_CONSTANT,
            Scalar::all(0.0),
        )?;
        self.palm.set_input(&blob(&square, side)?, "", 1.0, Scalar::default())?;
        let mut outs = Vector::<Mat>::new();
        self.palm.forward(&mut outs, &self.palm_out)?;
        let (mut regressors, mut scores) = (None, None);
        for out in &outs {
            let data = out.data_typed::<f32>()?;
            match data.len() {
                n if n == self.anchors.len() * 18 => regressors = Some(data.to_vec()),
                n if n == self.anchors.len() => scores = Some(data.to_vec()),
                _ => {}
            }
        }
        let (Some(regressors), Some(scores)) = (regressors, scores) else {
            anyhow::bail!("the palm model answered in a shape it does not have");
        };
        Ok(model::palms(&regressors, &scores, &lb, &self.anchors))
    }

    /// The hand in `roi`: how sure the model is there is one, and its 21
    /// points in `rgb`'s px with their depth.
    fn hand(&mut self, rgb: &Mat, roi: &Roi) -> anyhow::Result<(f32, [[f32; 3]; POINTS])> {
        let side = HAND_SIDE as i32;
        let map = Mat::from_slice_2d(&roi.crop())?;
        let mut crop = Mat::default();
        imgproc::warp_affine(
            rgb,
            &mut crop,
            &map,
            Size::new(side, side),
            imgproc::INTER_LINEAR,
            core::BORDER_CONSTANT,
            Scalar::all(0.0),
            core::AlgorithmHint::ALGO_HINT_DEFAULT,
        )?;
        self.hand.set_input(&blob(&crop, side)?, "", 1.0, Scalar::default())?;
        let mut outs = Vector::<Mat>::new();
        self.hand.forward(&mut outs, &self.hand_out)?;
        // The points, the score, the handedness and the points in metres,
        // in that order — as the zoo reads them.
        let read = outs.get(0)?;
        let read = read.data_typed::<f32>()?;
        let score = outs.get(1)?.data_typed::<f32>()?.first().copied().unwrap_or(0.0);
        anyhow::ensure!(read.len() == POINTS * 3, "the hand model answered {} numbers", read.len());
        let mut points = [[0.0_f32; 3]; POINTS];
        for (i, p) in points.iter_mut().enumerate() {
            *p = roi.back([read[3 * i], read[3 * i + 1], read[3 * i + 2]]);
        }
        Ok((score, points))
    }
}

/// `rgb` as the models take it: floats from 0 to 1, one image of
/// `side` by `side` by three channels.
fn blob(rgb: &Mat, side: i32) -> anyhow::Result<Mat> {
    let mut floats = Mat::default();
    rgb.convert_to(&mut floats, core::CV_32FC3, 1.0 / 255.0, 0.0)?;
    Ok(floats.reshape_nd(1, &[1, side, side, 3])?.try_clone()?)
}

/// The frame for the presenter's card: small, RGBA.
fn card_of(mirror: &Mat) -> anyhow::Result<(u32, u32, Vec<u8>)> {
    let h = CARD_W * mirror.rows() / mirror.cols().max(1);
    let mut small = Mat::default();
    imgproc::resize(mirror, &mut small, Size::new(CARD_W, h), 0.0, 0.0, imgproc::INTER_AREA)?;
    let mut rgba = Mat::default();
    imgproc::cvt_color_def(&small, &mut rgba, imgproc::COLOR_BGR2RGBA)?;
    Ok((CARD_W as u32, h as u32, rgba.data_bytes()?.to_vec()))
}

fn run(
    models: &mut Models,
    cap: &mut videoio::VideoCapture,
    camera: &str,
    stop: &AtomicBool,
    card: &AtomicBool,
    sink: &impl Fn(News) -> bool,
) -> anyhow::Result<()> {
    let file = camera.parse::<i32>().is_err();
    let (mut frame, mut mirror, mut rgb) = (Mat::default(), Mat::default(), Mat::default());
    let mut followed: Vec<Roi> = Vec::new();
    let mut n: u64 = 0;
    let (mut spent, mut frames, mut since) = (Duration::ZERO, 0_u32, Instant::now());
    while !stop.load(Ordering::Relaxed) {
        let began = Instant::now();
        if !cap.read(&mut frame)? || frame.empty() {
            anyhow::ensure!(file, "the camera stopped giving frames");
            // A video stands in for the camera: round again.
            cap.set(videoio::CAP_PROP_POS_FRAMES, 0.0)?;
            continue;
        }
        let read_at = Instant::now();
        core::flip(&frame, &mut mirror, 1)?;
        imgproc::cvt_color_def(&mirror, &mut rgb, imgproc::COLOR_BGR2RGB)?;

        // The hands followed first, then — when one is missing — what
        // the detector finds that none of them already is.
        let kept = followed.len();
        let mut rois = std::mem::take(&mut followed);
        if rois.len() < 2 && (rois.is_empty() || n.is_multiple_of(SEARCH_EVERY)) {
            for palm in models.palms(&rgb)? {
                let roi = Roi::of(&palm.keys, palm.middle());
                if rois.len() < 2 && !rois.iter().any(|r| r.same_hand(&roi)) {
                    rois.push(roi);
                }
            }
        }
        let (w, h) = (rgb.cols() as f32, rgb.rows() as f32);
        let mut hands = Vec::new();
        for (i, roi) in rois.iter().enumerate() {
            let (score, points) = models.hand(&rgb, roi)?;
            if score < if i < kept { KEEP } else { FIND } {
                continue;
            }
            let (keys, pivot) = model::palm_of(&points);
            let next = Roi::of(&keys, pivot);
            // Two squares that came to rest on one hand are one hand.
            if followed.iter().any(|r| r.same_hand(&next)) {
                continue;
            }
            followed.push(next);
            hands.push(Hand {
                points: points.map(|p| [p[0] / w, p[1] / h, p[2] / w]),
            });
        }
        let card = if card.load(Ordering::Relaxed) {
            Some(card_of(&mirror)?)
        } else {
            None
        };
        if !sink(News::Read(Reading {
            hands,
            size: (w as u32, h as u32),
            card,
        })) {
            break;
        }
        n += 1;

        spent += read_at.elapsed();
        frames += 1;
        if since.elapsed() >= REPORT_EVERY {
            let fps = f64::from(frames) / since.elapsed().as_secs_f64();
            let ms = spent.as_secs_f64() * 1000.0 / f64::from(frames.max(1));
            log::info!("hands: {fps:.0} frames a second, {ms:.1} ms of work a frame");
            (spent, frames, since) = (Duration::ZERO, 0, Instant::now());
        }
        if file {
            // At the pace a camera would give.
            std::thread::sleep(Duration::from_secs_f64(1.0 / 30.0).saturating_sub(began.elapsed()));
        }
    }
    Ok(())
}
