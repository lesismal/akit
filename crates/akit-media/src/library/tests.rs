//! fit's `medialib` tests ported (I141): `catalog_test.go`, `scan_test.go`, `library_test.go`
//! and `edges_test.go`, on fit's fixtures (`tests/data/medialib`, fit's `medialib/testdata`, made
//! by its `gen.sh`). Every wait is on the library's status or a callback, each with a deadline.
#![allow(clippy::arithmetic_side_effects)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use super::catalog::path_id;
use super::*;

const WAIT: Duration = Duration::from_secs(20);

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/medialib")
}

/// A directory of the test's own, removed when it is dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl TempDir {
    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, p: &str) -> PathBuf {
        self.0.join(p)
    }

    fn s(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

fn temp() -> TempDir {
    let n = NEXT.fetch_add(1, SeqCst);
    let p = std::env::temp_dir().join(format!("akit-medialib-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&p);
    fs::create_dir_all(&p).unwrap();
    // The path as the system resolves it (macOS's /var is /private/var).
    TempDir(fs::canonicalize(&p).unwrap())
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dst = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &dst);
        } else {
            fs::copy(e.path(), dst).unwrap();
        }
    }
}

/// fit's `copyLib`: the fixtures' library, copied for the test to change.
fn copy_lib() -> TempDir {
    let d = temp();
    copy_dir(&data().join("lib"), d.path());
    d
}

/// The libraries a test applied, removed when it ends, their threads waited for.
struct Opened {
    key: usize,
}

impl Drop for Opened {
    fn drop(&mut self) {
        let libs = libraries(self.key);
        remove(self.key);
        for l in libs {
            l.join(WAIT);
        }
    }
}

static KEYS: AtomicUsize = AtomicUsize::new(1 << 40);

fn open(libs: &[MediaLibrary]) -> (Opened, Arc<Library>) {
    let key = KEYS.fetch_add(1, SeqCst);
    apply(key, libs).unwrap();
    let l = lookup(key, &libs[0].name).unwrap();
    (Opened { key }, l)
}

fn lib(name: &str, roots: &[&str]) -> MediaLibrary {
    MediaLibrary {
        name: name.to_owned(),
        roots: roots.iter().map(|r| (*r).to_owned()).collect(),
        ..MediaLibrary::default()
    }
}

/// fit's `scanned`: waits until `l` has ended `n` scans in all, and returns its catalog.
fn scanned(l: &Library, n: u64) -> Arc<Catalog> {
    l.wait_status(WAIT, |st| st.scans >= n && !st.scanning)
        .unwrap_or_else(|| panic!("{}: {n} scans not ended: {:?}", l.name(), l.status()));
    l.catalog()
}

/// fit's `idle`: waits until `l` is neither scanning nor has a scan waiting.
fn idle(l: &Library) {
    l.wait_status(WAIT, |st| !st.scanning && !st.pending)
        .unwrap_or_else(|| panic!("{}: still scanning: {:?}", l.name(), l.status()));
}

/// fit's `rescan`: scans `l` again once it is idle, and waits for that scan.
fn rescan(l: &Library) -> Arc<Catalog> {
    idle(l);
    let before = l.status().scans;
    l.rescan();
    scanned(l, before + 1)
}

/// fit's `touch`: the file's time set later, so that a scan sees it changed.
fn touch(p: &Path) {
    let m = fs::metadata(p).unwrap();
    let later = m.modified().unwrap() + Duration::from_secs(3);
    fs::File::options()
        .write(true)
        .open(p)
        .unwrap()
        .set_modified(later)
        .unwrap();
}

/// fit's `get`: calls a method that calls back, and returns what it gave.
fn get<T: Send + 'static>(call: impl FnOnce(Box<dyn FnOnce(T) + Send>)) -> T {
    let (tx, rx) = mpsc::channel();
    call(Box::new(move |v| {
        let _ = tx.send(v);
    }));
    rx.recv_timeout(WAIT).expect("no callback")
}

fn ids(cat: &Catalog) -> String {
    cat.items
        .iter()
        .map(|it| it.id.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn problem_text(cat: &Catalog) -> String {
    cat.problems
        .iter()
        .map(|p| format!("{}: {}\n", p.path, p.error))
        .collect()
}

/// What ffprobe says of each media file of the fixtures (`lib.probe`).
struct Probed {
    duration: f64,
    streams: Vec<HashMap<String, String>>,
}

fn read_probe() -> HashMap<String, Probed> {
    let mut out: HashMap<String, Probed> = HashMap::new();
    for line in fs::read_to_string(data().join("lib.probe"))
        .unwrap()
        .lines()
    {
        let parts: Vec<&str> = line.split('|').collect();
        match parts[0] {
            "file" => {
                out.insert(
                    parts[1].to_owned(),
                    Probed {
                        duration: parts[2].parse().unwrap(),
                        streams: Vec::new(),
                    },
                );
            }
            "stream" => {
                let kv = parts[2..]
                    .iter()
                    .map(|p| {
                        let (k, v) = p.split_once('=').unwrap();
                        (k.to_owned(), v.to_owned())
                    })
                    .collect();
                out.get_mut(parts[1]).unwrap().streams.push(kv);
            }
            _ => {}
        }
    }
    out
}

/// Within 2ms of ffprobe's seconds.
fn near(d: Duration, seconds: &str) -> bool {
    seconds
        .parse::<f64>()
        .is_ok_and(|s| (d.as_secs_f64() - s).abs() <= 0.002)
}

/// fit's `TestCatalog`: every item of a library of ffmpeg-made files, with the durations and
/// codecs ffprobe gives, its renditions, audio, subtitles and poster grouped by their names or
/// its sidecar, and a broken file listed with its error.
#[test]
fn catalog() {
    let dir = copy_lib();
    let (_o, l) = open(&[lib("lib", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let av1 = path_id("clips/Av1 Opus");
    assert!(av1.starts_with("clips/Av1-Opus-"), "{av1}");
    assert_eq!(
        ids(&cat),
        format!("broken {av1} clips/late films/bbb sintel")
    );
    assert!(cat.problems.is_empty(), "{:?}", cat.problems);
    assert!(cat.truncated.is_empty() && cat.bytes > 0 && cat.scanned.is_some());

    let probes = read_probe();
    let mut checked = 0;
    for it in &cat.items {
        for r in it.renditions.iter().chain(&it.audio) {
            let Some(p) = probes.get(&r.file.path) else {
                assert_eq!(r.file.path, "broken.mp4", "not in lib.probe");
                continue;
            };
            checked += 1;
            assert!(r.error.is_empty(), "{}: {}", r.file.path, r.error);
            assert!(
                (r.duration.as_secs_f64() - p.duration).abs() <= 0.002,
                "{}: duration {:?}, ffprobe {}",
                r.file.path,
                r.duration,
                p.duration
            );
            let tracks: Vec<&Track> = r
                .tracks
                .iter()
                .filter(|t| t.kind == catalog::KIND_VIDEO || t.kind == catalog::KIND_AUDIO)
                .collect();
            assert_eq!(tracks.len(), p.streams.len(), "{}", r.file.path);
            for (t, s) in tracks.iter().zip(&p.streams) {
                let codec = s["codec_name"].replace("hevc", "h265");
                assert!(
                    t.codec == codec
                        && near(t.start, &s["start_time"])
                        && near(t.duration, &s["duration"]),
                    "{}: {t:?}, ffprobe {s:?}",
                    r.file.path
                );
                if t.kind == catalog::KIND_VIDEO {
                    assert_eq!(
                        (t.width.to_string(), t.height.to_string()),
                        (s["width"].clone(), s["height"].clone()),
                        "{}",
                        r.file.path
                    );
                } else {
                    assert_eq!(
                        (t.sample_rate.to_string(), t.channels.to_string()),
                        (s["sample_rate"].clone(), s["channels"].clone()),
                        "{}",
                        r.file.path
                    );
                }
                assert!(
                    !t.codec_string.is_empty() && t.samples > 0 && t.table_bytes > 0,
                    "{}: {t:?}",
                    r.file.path
                );
            }
            assert!(
                r.moov.is_some() && r.bitrate > 0 && !r.codecs.is_empty(),
                "{}: {r:?}",
                r.file.path
            );
        }
    }
    assert_eq!(checked, probes.len());

    let bbb = cat.item("films/bbb").unwrap();
    assert_eq!(
        (bbb.title.as_str(), bbb.root.as_str(), bbb.path.as_str()),
        ("bbb", dir.s().as_str(), "films/bbb")
    );
    assert!(bbb.error.is_empty() && bbb.sidecar.is_none());
    assert_eq!(bbb.renditions.len(), 2);
    assert_eq!(bbb.renditions[0].name, "360p");
    assert_eq!(bbb.renditions[0].width, 128);
    assert_eq!(bbb.renditions[1].name, "");
    assert_eq!(bbb.renditions[1].file.path, "films/bbb.mp4");
    // fit's says avc1.64000b of its own copy; the fixture's level is the file's.
    let codecs = bbb.renditions[0].codecs.clone();
    assert!(
        codecs.starts_with("avc1.6400") && codecs.ends_with(",mp4a.40.2"),
        "{codecs}"
    );
    assert_eq!(bbb.audio.len(), 1);
    assert_eq!(
        (
            bbb.audio[0].name.as_str(),
            bbb.audio[0].language.as_str(),
            bbb.audio[0].width
        ),
        ("audio-fr", "fr", 0)
    );
    assert_eq!(bbb.subtitles.len(), 2);
    let s = &bbb.subtitles[0];
    assert_eq!(
        (
            s.file.path.as_str(),
            s.format.as_str(),
            s.language.as_str(),
            s.name.as_str()
        ),
        ("films/bbb.en.srt", "srt", "en", "en")
    );
    assert!(!s.forced && s.error.is_empty());
    let s = &bbb.subtitles[1];
    assert_eq!(
        (s.file.path.as_str(), s.format.as_str(), s.language.as_str()),
        ("films/bbb.fr.forced.vtt", "vtt", "fr")
    );
    assert!(s.forced);
    assert_eq!(bbb.poster.as_ref().unwrap().path, "films/bbb.jpg");
    assert_eq!(bbb.duration, Duration::from_secs(1));
    let size: u64 = bbb.files().map(|f| f.size).sum();
    assert_eq!(bbb.size, size);

    let sintel = cat.item("sintel").unwrap();
    assert_eq!(sintel.title, "Sintel");
    assert_eq!(sintel.sidecar.as_ref().unwrap().path, "films/sintel.json");
    assert_eq!(sintel.poster.as_ref().unwrap().path, "films/art/sintel.png");
    assert_eq!(sintel.renditions.len(), 2);
    assert_eq!(
        (
            sintel.renditions[0].name.as_str(),
            sintel.renditions[0].tracks[0].codec
        ),
        ("hd", "h265")
    );
    assert_eq!(
        (
            sintel.renditions[1].name.as_str(),
            sintel.renditions[1].tracks[1].codec
        ),
        ("sd", "opus")
    );
    assert_eq!(sintel.subtitles.len(), 1);
    assert_eq!(
        (
            sintel.subtitles[0].name.as_str(),
            sintel.subtitles[0].language.as_str()
        ),
        ("中文", "zh")
    );
    assert!(sintel.subtitles[0].default);

    let late = cat.item("clips/late").unwrap();
    let a = &late.renditions[0].tracks[1];
    assert_eq!(
        (a.start, a.duration),
        (Duration::from_millis(178), Duration::from_millis(832))
    );

    let broken = cat.item("broken").unwrap();
    assert_eq!(
        broken.error,
        "not an MP4 file, or one cut short before its moov box"
    );
    assert!(broken.renditions.len() == 1 && broken.renditions[0].moov.is_none());
    assert!(cat.item("nothing").is_none());
    assert!(Arc::ptr_eq(&l.item("sintel").unwrap(), sintel));
    assert_eq!(l.roots(), vec![dir.s()]);

    // As the Admin API shows it.
    let v = catalog::compact(&sintel.to_value());
    for want in [
        r#""id":"sintel""#,
        r#""duration":1,"#,
        r#""name":"hd""#,
        r#""path":"films/sintel-hd.mp4""#,
        r#""codec":"h265""#,
        r#""audio":[]"#,
        r#""language":"zh""#,
    ] {
        assert!(v.contains(want), "{want} not in {v}");
    }
}

/// fit's `TestPathID`.
#[test]
fn path_ids() {
    for (rel, want) in [
        ("films/bbb", "films/bbb"),
        ("a_b/c~d.e-f", "a_b/c~d.e-f"),
        ("films/Big Buck Bunny", "films/Big-Buck-Bunny-"),
        ("电影/片", "x/x-"),
        (".hidden/x", "hidden/x-"),
    ] {
        let got = path_id(rel);
        assert!(
            got.starts_with(want) && catalog::valid_id(&got) && got == path_id(rel),
            "path_id({rel:?}) = {got:?}, want {want:?}..."
        );
        if want.ends_with('-') {
            assert_eq!(got.len(), want.len() + 8, "{got}: no hash");
        }
    }
    assert_ne!(path_id("a b"), path_id("a-b"));
    assert_ne!(path_id("a b"), path_id("a  b"));
    let long = format!("{}x y", "abcdefgh/".repeat(40));
    let got = path_id(&long);
    assert!(
        catalog::valid_id(&got) && got.starts_with("abcdefgh/"),
        "{got}"
    );
    let long = path_id(&format!("{} b", "a".repeat(100)));
    assert!(long.starts_with(&format!("{}-", "a".repeat(64))) && catalog::valid_id(&long));
    for id in [
        String::new(),
        ".x".into(),
        "a//b".into(),
        "a/".into(),
        "a".repeat(129),
        "a/".repeat(200),
        "a b".into(),
    ] {
        assert!(!catalog::valid_id(&id), "{id}");
    }
}

/// fit's `TestPatterns`.
#[test]
fn patterns() {
    for (pattern, rel, want) in [
        ("*.mp4", "a/b/c.mp4", true),
        ("*.mp4", "a/b/c.m4a", false),
        ("films/*.mp4", "films/a.mp4", true),
        ("films/*.mp4", "films/x/a.mp4", false),
        ("films/**/*.mp4", "films/a.mp4", true),
        ("films/**/*.mp4", "films/x/y/a.mp4", true),
        ("**/extras/**", "films/extras/a.mp4", true),
        ("**/extras/**", "films/extras", true),
        ("**/extras/**", "films/x/a.mp4", false),
        ("films/**", "films", true),
        ("a/b", "a", false),
    ] {
        assert_eq!(Pattern::new(pattern).matches(rel), want, "{pattern} {rel}");
    }
}

/// fit's `TestRescan`: files added, changed and removed are found after a rescan; files
/// unchanged are not probed again.
#[test]
fn rescans() {
    let dir = copy_lib();
    let (_o, l) = open(&[lib("lib", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let st = l.status();
    assert!(
        st.probe == 8 && st.probed == 8 && st.reused == 0 && st.files > 0,
        "{st:?}"
    );
    assert!(st.ended.unwrap() >= st.started.unwrap());
    let before = cat.item("films/bbb").unwrap().renditions[1].clone();
    fs::copy(dir.join("films/sintel-sd.mp4"), dir.join("clips/new.mp4")).unwrap();
    fs::copy(dir.join("films/sintel-hd.mp4"), dir.join("films/bbb.mp4")).unwrap();
    touch(&dir.join("films/bbb.mp4"));
    fs::remove_file(dir.join("clips/late.mp4")).unwrap();
    let cat = rescan(&l);
    let st = l.status();
    assert_eq!((st.probe, st.reused), (2, 6), "{st:?}");
    assert_eq!(
        cat.item("clips/new").unwrap().renditions[0].tracks[1].codec,
        "opus"
    );
    assert!(cat.item("clips/late").is_none());
    let after = &cat.item("films/bbb").unwrap().renditions[1];
    assert_eq!(after.tracks[0].codec, "h265");
    assert_ne!(after.file.size, before.file.size);
    assert_eq!(before.tracks[0].codec, "h264");

    fs::write(dir.join("clips/new.mp4"), "not a film at all").unwrap();
    let cat = rescan(&l);
    let it = cat.item("clips/new").unwrap();
    assert!(
        !it.error.is_empty() && it.renditions[0].error == it.error,
        "{it:?}"
    );
}

/// fit's `TestPathsOut`: a sidecar's path with `..`, an absolute one, and a symbolic link out of
/// the root are refused, unless `follow_symlinks`; links within the root are followed, and one
/// that loops is walked once.
#[test]
fn paths_out() {
    let dir = copy_lib();
    let out = temp();
    fs::copy(dir.join("films/bbb.mp4"), out.join("outside.mp4")).unwrap();
    fs::create_dir_all(out.join("more")).unwrap();
    fs::copy(dir.join("films/bbb.mp4"), out.join("more/deep.mp4")).unwrap();
    for (link, target) in [
        (
            dir.join("clips/inside.mp4"),
            dir.join("films/sintel-sd.mp4"),
        ),
        (dir.join("clips/out.mp4"), out.join("outside.mp4")),
        (dir.join("clips/outdir"), out.join("more")),
        (dir.join("clips/loop"), dir.path().to_path_buf()),
        (dir.join("clips/gone.mp4"), out.join("nothing.mp4")),
    ] {
        std::os::unix::fs::symlink(target, link).unwrap();
    }
    let side = r#"{"title": "out", "renditions": [{"file": "../../outside.mp4"}, {"file": "/etc/passwd"}, {"file": "nothing.mp4"}, {"file": "sintel.json"}], "audio": [{"file": "bbb.audio-fr.m4a"}, {"file": "bbb.audio-fr.m4a"}], "subtitles": [{"file": "x\\y.srt"}], "poster": "sintel-hd.mp4"}"#;
    fs::write(dir.join("films/out.json"), side).unwrap();
    let (_o, l) = open(&[lib("lib", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    assert!(
        cat.item("clips/inside")
            .is_some_and(|it| it.error.is_empty())
    );
    for id in ["clips/out", "clips/outdir/deep", "clips/loop/films/bbb"] {
        assert!(cat.item(id).is_none(), "{id} is listed");
    }
    let mut problems: HashMap<&str, String> = HashMap::new();
    for p in &cat.problems {
        let e = problems.entry(p.path.as_str()).or_default();
        e.push_str(&p.error);
        e.push_str("; ");
    }
    for (path, want) in [
        ("clips/out.mp4", "a symbolic link out of the root, refused"),
        ("clips/outdir", "a symbolic link out of the root, refused"),
        ("clips/gone.mp4", "a symbolic link that leads nowhere"),
        (
            "films/out.json",
            r#""../../outside.mp4" leads out of the sidecar's directory"#,
        ),
        ("films/out.json", r#""/etc/passwd" leads out"#),
        (
            "films/out.json",
            r#""nothing.mp4" is not a file the library looks at"#,
        ),
        ("films/out.json", r#""sintel.json" is not a media file"#),
        ("films/out.json", r#""x\\y.srt" leads out"#),
        (
            "films/out.json",
            r#"poster "sintel-hd.mp4" is not a picture"#,
        ),
        (
            "films/out.json",
            r#"file "bbb.audio-fr.m4a" is named by a sidecar already"#,
        ),
    ] {
        let got = problems.get(path).cloned().unwrap_or_default();
        assert!(got.contains(want), "{path}: {got:?}, want {want:?}");
    }
    let it = cat.item("films/out").unwrap();
    assert!(it.error.is_empty() && it.audio.len() == 1, "{it:?}");
    assert!(cat.item("films/bbb").unwrap().audio.is_empty());

    // A file changed into a link out of its root after the scan.
    let r = cat.item("clips/inside").unwrap().renditions[0].clone();
    assert!(get(|cb| l.open(&r.file, cb)).is_ok());
    let link = dir.join("clips/inside.mp4");
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(out.join("outside.mp4"), &link).unwrap();
    assert_eq!(get(|cb| l.open(&r.file, cb)).err(), Some(Error::Outside));

    // With follow_symlinks.
    let mut c = lib("follow", &[&dir.s()]);
    c.follow_symlinks = true;
    let (_f, f) = open(&[c]);
    let cat = scanned(&f, 1);
    for id in ["clips/out", "clips/outdir/deep", "clips/inside"] {
        assert!(cat.item(id).is_some_and(|it| it.error.is_empty()), "{id}");
    }
}

/// fit's `TestBounds`: each bound of a scan holds.
#[test]
fn bounds() {
    let dir = copy_lib();
    let mut opened = Vec::new();
    let mut scan = |name: &str, edit: &dyn Fn(&mut MediaLibrary)| {
        let mut c = lib(name, &[&dir.s()]);
        edit(&mut c);
        let (o, l) = open(&[c]);
        let cat = scanned(&l, 1);
        opened.push(o);
        (l, cat)
    };
    let (_, cat) = scan("items", &|c| c.max_items = 2);
    assert!(cat.items.len() == 2 && cat.truncated.contains("max_items"));
    let (_, cat) = scan("files", &|c| c.max_files = 3);
    assert!(
        cat.items.is_empty() && cat.truncated.contains("max_files"),
        "{:?}",
        cat.truncated
    );
    let (_, cat) = scan("bytes", &|c| c.max_catalog_bytes = 6000);
    assert!(
        !cat.items.is_empty()
            && cat.items.len() < 5
            && cat.bytes <= 6000
            && cat.truncated.contains("max_catalog_bytes"),
        "{} items, {} bytes, {:?}",
        cat.items.len(),
        cat.bytes,
        cat.truncated
    );
    let (_, cat) = scan("sidebytes", &|c| c.max_catalog_bytes = 100);
    assert!(
        cat.truncated
            .contains("the sidecars pass max_catalog_bytes")
    );
    let (last, cat) = scan("sidecar", &|c| {
        c.max_sidecar_bytes = 100;
        c.max_subtitle_bytes = 50;
    });
    assert_eq!(cat.problems.len(), 1, "{:?}", cat.problems);
    assert!(cat.problems[0].error.contains("past max_sidecar_bytes"));
    assert!(cat.item("sintel").is_none() && cat.item("films/sintel-hd").is_some());
    let subs = cat.item("films/bbb").unwrap().subtitles.clone();
    assert!(subs[0].error.contains("past max_subtitle_bytes") && subs[1].error.is_empty());
    assert_eq!(
        get(|cb| last.subtitle(&subs[0], cb)).err(),
        Some(Error::NotServed)
    );

    let (_, cat) = scan("patterns", &|c| {
        c.files = vec!["*.mp4".into(), "*.json".into(), "*.srt".into()];
        c.exclude = vec!["clips/**".into(), "*-sd.mp4".into()];
    });
    assert_eq!(ids(&cat), "broken films/bbb sintel");
    let bbb = cat.item("films/bbb").unwrap();
    assert!(bbb.audio.is_empty() && bbb.subtitles.len() == 1);
    assert_eq!(cat.problems.len(), 2, "{:?}", cat.problems);
    assert!(
        cat.problems[0]
            .error
            .contains(r#""sintel-sd.mp4" is not a file the library looks at"#)
    );
    assert!(
        cat.problems[1]
            .error
            .contains(r#"poster "art/sintel.png" is not a file the library looks at"#)
    );

    // Problems past MAX_PROBLEMS are counted.
    let many = temp();
    for i in 0..catalog::MAX_PROBLEMS + 5 {
        fs::write(many.join(&format!("s{i}.json")), "{").unwrap();
    }
    let (_m, l) = open(&[lib("many", &[&many.s()])]);
    let cat = scanned(&l, 1);
    assert_eq!(
        (cat.problems.len(), cat.more_problems),
        (catalog::MAX_PROBLEMS, 5)
    );
}

/// fit's `TestRoots`: a root missing, or not a directory, is reported, and the others scanned;
/// an ID two roots make is listed once.
#[test]
fn roots() {
    let dir = copy_lib();
    let other = temp();
    fs::create_dir_all(other.join("films")).unwrap();
    fs::copy(dir.join("films/bbb.mp4"), other.join("films/bbb.mp4")).unwrap();
    fs::write(other.join("sd.json"), r#"{"id": "sintel"}"#).unwrap();
    fs::copy(dir.join("films/bbb.mp4"), other.join("sd.mp4")).unwrap();
    let missing = dir.path().join("../missing").to_string_lossy().into_owned();
    let notes = dir.join("notes.txt").to_string_lossy().into_owned();
    let (_o, l) = open(&[lib("lib", &[&dir.s(), &missing, &notes, &other.s()])]);
    let cat = scanned(&l, 1);
    assert_eq!(cat.items.len(), 5);
    assert_eq!(cat.item("films/bbb").unwrap().root, dir.s());
    assert_eq!(cat.item("sintel").unwrap().root, dir.s());
    let got: String = cat
        .problems
        .iter()
        .map(|p| format!("{}: {}: {}\n", p.root, p.path, p.error))
        .collect();
    for want in [
        "missing: : the root:",
        "notes.txt: : the root: not a directory",
        r#"films/bbb: the ID "films/bbb" is taken by"#,
        r#"sd: the ID "sintel" is taken by"#,
    ] {
        assert!(got.contains(want), "problems:\n{got}\nwant {want:?}");
    }
}

/// fit's `TestDeepAndOdd`: no deeper than `MAX_DEPTH`, names starting with a dot and files of no
/// kind skipped, and a sidecar that does not parse listed as a problem.
#[test]
fn deep_and_odd() {
    let dir = temp();
    let mut deep = dir.path().to_path_buf();
    for _ in 0..=MAX_DEPTH {
        deep = deep.join("d");
    }
    fs::create_dir_all(&deep).unwrap();
    let src = data().join("lib/films/bbb.mp4");
    for p in [
        deep.join("deep.mp4"),
        dir.join(".hidden.mp4"),
        dir.join("a.mp4"),
    ] {
        fs::copy(&src, p).unwrap();
    }
    let long = format!(r#"{{"title": "{}"}}"#, "x".repeat(2000));
    for (name, body) in [
        ("bad.json", r#"{"unknown": 1}"#),
        ("two.json", "{} {}"),
        ("id.json", r#"{"id": "../x"}"#),
        ("long.json", long.as_str()),
        (
            "a.json",
            r#"{"title": "A", "audio": [{"file": "a.mp4", "name": "main", "language": "en", "default": true}]}"#,
        ),
        ("a.unknown.vt", "x"),
        ("a..en.default.sdh.srt", "x"),
        ("empty.json", r#"{"title": "E"}"#),
        ("a.poster.jpg", "x"),
        ("a.jpg", "x"),
    ] {
        fs::write(dir.join(name), body).unwrap();
    }
    let (_o, l) = open(&[lib("deep", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let problems = problem_text(&cat);
    for want in [
        "deeper than 32 directories",
        r#"bad.json: the sidecar: json: unknown field "unknown""#,
        "two.json: the sidecar: more than one JSON value",
        r#"id.json: the sidecar: id "../x" is not an item's ID"#,
        "long.json: the sidecar: a title, name or language is not UTF-8",
    ] {
        assert!(
            problems.contains(want),
            "problems:\n{problems}\nwant {want:?}"
        );
    }
    let a = cat.item("a").unwrap();
    assert_eq!(a.subtitles.len(), 1);
    let s = &a.subtitles[0];
    assert!(s.language == "en" && s.default && s.name == "sdh", "{s:?}");
    let e = cat.item("empty").unwrap();
    assert_eq!(e.title, "E");
    assert_eq!(
        e.error,
        "no media file: the sidecar names none, and none is named after it"
    );
    assert_eq!(cat.items.len(), 2);
    assert!(a.title == "A" && a.audio.len() == 1 && a.renditions.is_empty());
    assert!(a.audio[0].default && a.audio[0].language == "en" && a.audio[0].name == "main");
    assert_eq!(a.poster.as_ref().unwrap().path, "a.jpg");
    assert_eq!(a.duration, Duration::from_secs(1));
}

/// fit's `TestChangingFile`: a file that changes while it is probed again is listed with that
/// error, to be probed again at the next scan.
#[test]
fn changing_file() {
    let dir = copy_lib();
    let mut c = lib("lib", &[&dir.s()]);
    c.max_probes = 1;
    let (_o, l) = open(&[c]);
    scanned(&l, 1);
    let p = dir.join("films/bbb.mp4");
    // Each probe changes the file first, as a copy under way would.
    let n = Arc::new(AtomicUsize::new(0));
    let (hp, hn) = (p.clone(), Arc::clone(&n));
    l.set_probe_hook(Some(Arc::new(move || {
        let k = hn.fetch_add(1, SeqCst) as u64 + 1;
        let later = SystemTime::now() + Duration::from_secs(3600 * k);
        if let Ok(f) = fs::File::options().write(true).open(&hp) {
            let _ = f.set_modified(later);
        }
    })));
    touch(&p);
    let cat = rescan(&l);
    let r = &cat.item("films/bbb").unwrap().renditions[1];
    assert!(
        r.error.contains("changed while it was probed") && r.transient,
        "{r:?}"
    );
    l.set_probe_hook(None);
    let cat = rescan(&l);
    assert!(
        cat.item("films/bbb").unwrap().renditions[1]
            .error
            .is_empty()
    );
}

/// fit's `TestTable`, for akit's movies: read on the pool once for every call that waits, kept
/// within `max_table_bytes`, and dropped when its file changes.
#[test]
fn movies() {
    let dir = copy_lib();
    let (_o, l) = open(&[lib("lib", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let bbb = cat.item("films/bbb").unwrap().renditions[1].clone();

    // Many at once: one read, every caller called.
    let (tx, rx) = mpsc::channel();
    for _ in 0..50 {
        let tx = tx.clone();
        l.movie(&bbb, move |m| {
            let _ = tx.send(m);
        });
    }
    let got: Vec<Arc<akio_media::mp4::Movie>> = (0..50)
        .map(|_| rx.recv_timeout(WAIT).unwrap().unwrap())
        .collect();
    for m in &got {
        assert!(Arc::ptr_eq(m, &got[0]));
    }
    assert_eq!(got[0].tracks[0].index.len(), 25);
    assert_eq!(got[0].tracks[1].index.len(), 48);
    // Kept: called at once, on this thread.
    let (tx, rx) = mpsc::channel();
    l.movie(&bbb, move |m| {
        let _ = tx.send(m.is_ok());
    });
    assert_eq!(
        rx.try_recv(),
        Ok(true),
        "a movie kept is not handed over at once"
    );
    let st = l.tables();
    assert_eq!(st.tables, 1);
    assert_eq!(st.max, akit_config::medialib::DEFAULT_MAX_TABLE_BYTES);
    assert!(
        st.hits + st.misses == 51 && st.hits >= 1 && st.misses >= 1,
        "{st:?}"
    );

    // Bounded: room for one movie; the least recently used goes.
    let hd = cat.item("sintel").unwrap().renditions[0].clone();
    let one = st.bytes;
    l.resize_tables(one + 100);
    get(|cb| l.movie(&hd, cb)).unwrap();
    assert_eq!(l.tables().tables, 1);
    l.resize_tables(10);
    assert_eq!(get(|cb| l.movie(&bbb, cb)).err(), Some(Error::TooLarge));
    l.resize_tables(akit_config::medialib::DEFAULT_MAX_TABLE_BYTES);

    // What cannot be read.
    let broken = cat.item("broken").unwrap().renditions[0].clone();
    assert_eq!(get(|cb| l.movie(&broken, cb)).err(), Some(Error::NotServed));

    // The file changed since the scan: refused, and its movie dropped.
    get(|cb| l.movie(&bbb, cb)).unwrap();
    touch(&dir.join("films/bbb.mp4"));
    assert_eq!(get(|cb| l.open(&bbb.file, cb)).err(), Some(Error::Changed));
    let st = l.tables();
    assert_eq!((st.tables, st.bytes), (0, 0), "{st:?}");
    assert_eq!(get(|cb| l.movie(&bbb, cb)).err(), Some(Error::Changed));
    // The next scan has it as it is, and keeps none of the old.
    let cat = rescan(&l);
    let bbb = cat.item("films/bbb").unwrap().renditions[1].clone();
    get(|cb| l.movie(&bbb, cb)).unwrap();
    fs::remove_file(dir.join("films/bbb.mp4")).unwrap();
    rescan(&l);
    assert_eq!(l.tables().tables, 0);
    assert_eq!(get(|cb| l.open(&bbb.file, cb)).err(), Some(Error::Changed));
}

/// fit's `TestTableBusy`: busy when the pool refuses a movie's read, when movies being read hold
/// `max_table_bytes`, and when too many wait for one; a read dropped while it runs hands its
/// movie to those waiting and keeps nothing.
#[test]
fn movies_busy() {
    let dir = copy_lib();
    let (_o, l) = open(&[lib("lib", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let bbb = cat.item("films/bbb").unwrap().renditions[1].clone();
    let hd = cat.item("sintel").unwrap().renditions[0].clone();

    // A pool whose one worker is held.
    let pool = Pool::new(1, 1, Duration::from_secs(60), "t");
    let (release_tx, release) = mpsc::channel::<()>();
    let (held_tx, held) = mpsc::channel::<()>();
    assert!(pool.submit(
        Box::new(move || {
            held_tx.send(()).unwrap();
            let _ = release.recv();
        }),
        None
    ));
    held.recv_timeout(WAIT).unwrap();
    l.set_pool(pool.clone()).close();

    let (tx, results) = mpsc::channel();
    for _ in 0..=MAX_WAITERS {
        let tx = tx.clone();
        l.movie(&bbb, move |m| {
            let _ = tx.send(m.err());
        });
    }
    assert_eq!(results.recv_timeout(WAIT).unwrap(), Some(Error::Busy));
    // The queue holds the read of bbb: hd is refused.
    assert_eq!(get(|cb| l.movie(&hd, cb)).err(), Some(Error::Busy));
    // Room for the movie being read alone.
    l.resize_tables(l.tables().bytes);
    assert_eq!(get(|cb| l.movie(&hd, cb)).err(), Some(Error::Busy));
    l.resize_tables(akit_config::medialib::DEFAULT_MAX_TABLE_BYTES);
    // Dropped while it is read: those waiting get it, and it is not kept.
    l.tables.drop_file(&bbb.file.abs);
    release_tx.send(()).unwrap();
    for _ in 0..MAX_WAITERS {
        assert_eq!(results.recv_timeout(WAIT).unwrap(), None);
    }
    let st = l.tables();
    assert_eq!((st.tables, st.bytes), (0, 0), "{st:?}");
    pool.close();
}

/// fit's `TestSubtitles`: a WebVTT file served as it is, an SRT file turned into WebVTT.
#[test]
fn subtitles() {
    let dir = copy_lib();
    let (_o, l) = open(&[lib("lib", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let subs = cat.item("films/bbb").unwrap().subtitles.clone();
    let srt = get(|cb| l.subtitle(&subs[0], cb)).unwrap();
    assert_eq!(
        String::from_utf8(srt).unwrap(),
        "WEBVTT\n\n00:00:00.100 --> 00:00:00.900\nHello\n\n2\n00:00:01.000 --> 00:00:02.500\nWorld --&gt; on\n"
    );
    let vtt = get(|cb| l.subtitle(&subs[1], cb)).unwrap();
    assert_eq!(vtt, b"WEBVTT\n\n00:00.100 --> 00:00.900\nBonjour\n");
    let zh = get(|cb| l.subtitle(&cat.item("sintel").unwrap().subtitles[0], cb)).unwrap();
    assert!(
        String::from_utf8(zh)
            .unwrap()
            .contains("00:00:00.000 --> 00:00:01.000\n你好")
    );
    fs::write(dir.join("films/bbb.en.srt"), "no cues").unwrap();
    assert_eq!(
        get(|cb| l.subtitle(&subs[0], cb)).err(),
        Some(Error::Changed)
    );
    let s = rescan(&l).item("films/bbb").unwrap().subtitles[0].clone();
    let e = get(|cb| l.subtitle(&s, cb)).unwrap_err();
    assert!(e.to_string().contains("not an SRT file"), "{e}");

    for (format, input, want) in [
        ("vtt", "\u{feff}WEBVTT", "WEBVTT"),
        ("vtt", "WEBVTT - a title\n", "WEBVTT - a title\n"),
        ("vtt", "WEBVTTX\n", ""),
        ("vtt", "1\n00:00:01,000 --> 00:00:02,000\n", ""),
        (
            "srt",
            "junk\r1\r00:00:01.5 --> 00:00:02.25\rtext\r",
            "WEBVTT\n\n00:00:01.500 --> 00:00:02.250\ntext\n",
        ),
        ("ass", "x", ""),
    ] {
        match to_webvtt(format, input.as_bytes()) {
            Ok(b) => assert_eq!(String::from_utf8(b).unwrap(), want, "{format} {input:?}"),
            Err(e) => assert!(want.is_empty(), "{format} {input:?}: {e}"),
        }
    }
}

/// fit's `TestReload`: a library changed in place, scanned again when its roots or patterns
/// change, its pool replaced when its bounds do; one a reload leaves out removed, its methods
/// refused; a config that does not compile changes nothing.
#[test]
fn reload() {
    let dir = copy_lib();
    let other_dir = temp();
    let c = lib("lib", &[&dir.s()]);
    let (o, l) = open(&[c.clone(), lib("other", &[&other_dir.s()])]);
    let key = o.key;
    let other = lookup(key, "other").unwrap();
    scanned(&l, 1);
    scanned(&other, 1);
    let pool = l.pool();

    // Unchanged: nothing happens.
    apply(key, &[c.clone(), lib("other", &[&other_dir.s()])]).unwrap();
    assert!(Arc::ptr_eq(&lookup(key, "lib").unwrap(), &l));
    assert!(l.pool().same(&pool) && !l.status().pending);

    // New patterns: scanned again, in place.
    idle(&l);
    let scans = l.status().scans;
    let mut c2 = c.clone();
    c2.exclude = vec!["clips".into()];
    c2.file_workers = 2;
    c2.scan_interval = Duration::from_secs(3600);
    apply(key, &[c2.clone()]).unwrap();
    assert!(Arc::ptr_eq(&lookup(key, "lib").unwrap(), &l));
    assert!(!l.pool().same(&pool) && l.pool().stats().workers == 2);
    let cat = scanned(&l, scans + 1);
    assert!(cat.item("clips/late").is_none());
    assert!(pool.wait(WAIT), "the pool replaced ends");

    // Removed: refused, and its scanner ends.
    assert!(lookup(key, "other").is_none() && other.removed());
    assert!(other.join(WAIT));
    assert!(!other.rescan());
    let r = cat.item("films/bbb").unwrap().renditions[0].clone();
    assert_eq!(
        get(|cb| other.open(&r.file, cb)).err(),
        Some(Error::Removed)
    );
    assert_eq!(other.open_file(&r.file).err(), Some(Error::Removed));
    assert_eq!(get(|cb| other.movie(&r, cb)).err(), Some(Error::Removed));
    assert!(!other.submit(Box::new(|| {}), None));

    // Configs that do not compile change nothing.
    let mut bad_pattern = lib("x", &[&dir.s()]);
    bad_pattern.files = vec!["[".into()];
    for bad in [
        vec![c2.clone(), c2.clone()],
        vec![lib("x", &[])],
        vec![lib("x", &[" "])],
        vec![bad_pattern],
    ] {
        assert!(apply(key, &bad).is_err(), "{bad:?} applied");
    }
    assert!(Arc::ptr_eq(&lookup(key, "lib").unwrap(), &l) && !l.removed());
    apply(key, &[]).unwrap();
    assert!(lookup(key, "lib").is_none() && l.removed());
}

/// fit's `TestRole`, its metrics: what a library counts, as `/metrics` shows it.
#[test]
fn metrics() {
    let dir = copy_lib();
    let (o, a) = open(&[lib("a", &[&dir.s()]), lib("b", &[&dir.s()])]);
    scanned(&a, 1);
    scanned(&lookup(o.key, "b").unwrap(), 1);
    let mut w = akit_observe::metrics::TextWriter::new(Vec::new());
    write_metrics(&mut w, &libraries(o.key));
    let text = String::from_utf8(w.out).unwrap();
    for want in [
        r#"akit_media_library_items{library="a",state="ok"} 4"#,
        r#"akit_media_library_items{library="a",state="error"} 1"#,
        r#"akit_media_library_scans_total{library="b"} 1"#,
        r#"akit_media_library_probes_total{library="a",result="error"} 1"#,
        r#"akit_media_library_pool_jobs{library="a",state="queued"} 0"#,
        r#"akit_media_library_pool_refused_total{library="a",reason="expired"} 0"#,
        r#"akit_media_library_catalog_bytes{library="a"}"#,
        r#"akit_media_library_table_bytes{library="a"} 0"#,
        r#"akit_media_library_table_lookups_total{library="a",result="hit"} 0"#,
        r#"akit_media_library_table_lookups_total{library="b",result="miss"} 0"#,
    ] {
        assert!(text.contains(want), "metrics lack {want}:\n{text}");
    }
}

/// fit's `TestOddFiles`: a file of a codec the library does not serve is listed with its error;
/// a file cut short in its media data too, as akio-media's reader checks every chunk against the
/// file's size when it parses the `moov` (fib's reads the tables later).
#[test]
fn odd_files() {
    let dir = temp();
    fs::copy(data().join("odd/mjpeg.mp4"), dir.join("mjpeg.mp4")).unwrap();
    let b = fs::read(data().join("lib/films/bbb.360p.mp4")).unwrap();
    fs::write(dir.join("cut.mp4"), &b[..b.len() / 2]).unwrap();
    let (_o, l) = open(&[lib("odd", &[&dir.s()])]);
    let cat = scanned(&l, 1);
    let it = cat.item("mjpeg").unwrap();
    assert!(
        it.error.starts_with("no H.264, H.265 or AV1 video"),
        "{it:?}"
    );
    assert_eq!(it.renditions[0].tracks[0].codec, "unknown");
    let cut = cat.item("cut").unwrap();
    assert_eq!(cut.error, "a malformed MP4 file", "{cut:?}");
    assert_eq!(l.name(), "odd");
}

/// fit's `TestOpenFile`: what is not a regular file is refused, a file gone is changed.
#[test]
fn open_files() {
    let dir = temp();
    let f = |abs: PathBuf| File {
        root: String::new(),
        path: String::new(),
        size: 0,
        modified: SystemTime::UNIX_EPOCH,
        abs,
        real: dir.path().to_path_buf(),
    };
    assert_eq!(
        files::open_file(&f(dir.path().to_path_buf()), false).err(),
        Some(Error::Io("not a regular file".into()))
    );
    assert_eq!(
        files::open_file(&f(dir.join("none")), false).err(),
        Some(Error::Changed)
    );
}

/// fit's `TestInterval`: scans every `scan_interval`, a new interval taken at once.
#[test]
fn interval() {
    let dir = temp();
    let mut c = lib("lib", &[&dir.s()]);
    c.scan_interval = Duration::from_secs(3600);
    let (_o, l) = open(&[c.clone()]);
    scanned(&l, 1);
    // The config reader's minimum is 10s; the library takes what it is given.
    let mut s = Settings::compile(&c).unwrap();
    s.interval = Duration::from_millis(20);
    for _ in 0..3 {
        l.apply(s.clone());
    }
    scanned(&l, 4);
}

/// The Admin API's endpoints (fit's `admin/medialib_test.go`).
#[test]
fn admin_endpoints() {
    let dir = copy_lib();
    let (o, l) = open(&[lib("lib", &[&dir.s()])]);
    scanned(&l, 1);
    let body = |r: akio_http::Response| (r.status, String::from_utf8(r.body).unwrap());
    let (st, b) = body(admin::route(o.key, "", true, false, ""));
    assert!(
        st == 200 && b.contains(r#""name":"lib""#) && b.contains(r#""items":5"#),
        "{b}"
    );
    let (st, b) = body(admin::route(o.key, "/lib", true, false, "limit=2&offset=1"));
    assert!(
        st == 200 && b.contains(r#""matched":5"#) && b.contains(r#""limit":2"#),
        "{b}"
    );
    assert_eq!(b.matches(r#""renditions":"#).count(), 2, "{b}");
    let (_, b) = body(admin::route(o.key, "/lib", true, false, "codec=h265"));
    assert!(
        b.contains(r#""matched":1"#) && b.contains(r#""id":"sintel""#),
        "{b}"
    );
    let (_, b) = body(admin::route(o.key, "/lib", true, false, "error=true"));
    assert!(
        b.contains(r#""matched":1"#) && b.contains(r#""id":"broken""#),
        "{b}"
    );
    let (_, b) = body(admin::route(
        o.key,
        "/lib",
        true,
        false,
        "q=BBB&min_height=96",
    ));
    assert!(
        b.contains(r#""matched":1"#) && b.contains(r#""height":96"#),
        "{b}"
    );
    for q in [
        "limit=0",
        "limit=1001",
        "offset=x",
        "min_height=-1",
        "error=maybe",
    ] {
        assert_eq!(
            admin::route(o.key, "/lib", true, false, q).status,
            400,
            "{q}"
        );
    }
    let (st, b) = body(admin::route(o.key, "/lib/items/films/bbb", true, false, ""));
    assert!(st == 200 && b.contains(r#""id":"films/bbb""#), "{b}");
    assert_eq!(
        admin::route(o.key, "/lib/items/none", true, false, "").status,
        404
    );
    assert_eq!(admin::route(o.key, "/nope", true, false, "").status, 404);
    assert_eq!(
        admin::route(o.key, "/lib/rescan", true, false, "").status,
        405
    );
    let (st, b) = body(admin::route(o.key, "/lib/rescan", false, true, ""));
    assert!(st == 202 && b.contains(r#""queued":"#), "{b}");
    assert_eq!(admin::route(o.key, "/lib", false, true, "").status, 405);
    assert_eq!(
        admin::route(o.key, "/lib/what", true, false, "").status,
        404
    );
}
