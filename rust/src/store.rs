//! Durable checkpoint storage: the Rust half of `storage.set(CHECKPOINT_KEY, …)`.
//!
//! One file, rewritten in full after every settled trainee. No log, no history,
//! no schema version — a checkpoint answers exactly one question ("what did this
//! batch already write to the portal?") and only the latest answer is useful.
//!
//! Timestamps in the file come from the engine's `now_ms`, the epoch-millisecond
//! strings `report.rs` writes, not the TS original's ISO 8601. The two are not
//! interchangeable, which is the mapping `checkpoint.rs` warns the extension
//! side about.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::checkpoint::{BatchCheckpoint, CheckpointWriter};

/// Default file name, a sibling of the job file that started the batch.
pub const DEFAULT_FILE_NAME: &str = "dssp.checkpoint.json";

/// Overrides the location, following the `DSSP_*` convention in `worker.rs`.
pub const PATH_ENV: &str = "DSSP_CHECKPOINT";

pub struct CheckpointStore {
    path: PathBuf,
}

/// A checkpoint as it was on disk, plus what the file itself testifies to.
pub struct StoredCheckpoint {
    pub checkpoint: BatchCheckpoint,
    /// Whether the build that wrote this file recorded the trainee it was
    /// submitting.
    ///
    /// Load-bearing, and not the same question as `checkpoint.in_flight.is_none()`:
    /// `#[serde(default)]` makes an absent key and a null one identical once the
    /// struct exists. A build without the field wrote only at settle points, so a
    /// crash mid-submission leaves the trainee it was working on at the head of
    /// `pending` — where a reader with no marker to consult would take it for
    /// never-sent and submit it a second time. The key's *presence* is the only
    /// thing that separates those files from current ones.
    pub tracks_in_flight: bool,
}

impl CheckpointStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The file a batch run should use: `DSSP_CHECKPOINT` when set, otherwise a
    /// fixed name beside the job file.
    pub fn for_job(job_path: &Path) -> Self {
        Self::new(resolve_path(job_path, std::env::var_os(PATH_ENV)))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read the last checkpoint, if any.
    ///
    /// A missing file is `Ok(None)` — the ordinary case on a first run. A file
    /// that exists but cannot be read or parsed is an `Err`, deliberately: the
    /// one conclusion recovery must never draw from a corrupt checkpoint is that
    /// nothing was submitted.
    pub fn load(&self) -> Result<Option<BatchCheckpoint>, String> {
        Ok(self.load_with_provenance()?.map(|stored| stored.checkpoint))
    }

    /// Read the last checkpoint together with what the *file* can testify to.
    ///
    /// [`Self::load`] wraps this, so call sites that do not care about
    /// provenance are unchanged. Recovery is the one that does: whether the
    /// build that wrote a file could record a submission in flight decides
    /// whether its queue is evidence that those trainees were never sent.
    pub fn load_with_provenance(&self) -> Result<Option<StoredCheckpoint>, String> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };

        let unreadable = |e: serde_json::Error| format!("{}: unreadable checkpoint: {e}", self.path.display());

        // Parsed once as loose JSON, because the typed parse below cannot answer
        // the question: `#[serde(default)]` makes an absent `in_flight` key and a
        // null one identical once the struct exists, and telling those apart is
        // the whole point of asking.
        let raw: serde_json::Value = serde_json::from_str(&text).map_err(unreadable)?;
        let tracks_in_flight = raw
            .as_object()
            .is_some_and(|object| object.contains_key("in_flight"));

        let checkpoint = serde_json::from_str(&text).map_err(unreadable)?;

        Ok(Some(StoredCheckpoint {
            checkpoint,
            tracks_in_flight,
        }))
    }

    /// Rewrite the checkpoint file in one step.
    ///
    /// Through a temporary file and a rename, because `fs::write` truncates in
    /// place: a process killed mid-write would leave a half-written file that
    /// the next start cannot parse, and recovery would then have to treat a
    /// batch that did submit as one that never ran. The extension gets this for
    /// free from `storage.set`, which is the only reason the TS side has no
    /// equivalent dance. `rename(2)` on POSIX is atomic within one directory, so
    /// a reader sees either the previous checkpoint or the new one.
    pub fn save(&self, checkpoint: &BatchCheckpoint) -> Result<(), String> {
        let json = serde_json::to_string_pretty(checkpoint)
            .map_err(|e| format!("could not serialize checkpoint: {e}"))?;

        let temp = self.temp_path();

        if let Err(e) = write_and_sync(&temp, &json) {
            let _ = fs::remove_file(&temp);

            return Err(format!("{}: {e}", temp.display()));
        }

        fs::rename(&temp, &self.path).map_err(|e| {
            let _ = fs::remove_file(&temp);

            format!("{}: {e}", self.path.display())
        })?;

        // Durability, not atomicity. The rename is already visible to any other
        // process; this is what makes it survive a power loss. Without it the
        // loss can take the rename itself, and recovery then reads the *previous*
        // checkpoint — one whose `pending` may still name a trainee this batch
        // already submitted, which is a licence to submit it twice. Ignored
        // because a directory cannot be opened as a file everywhere, and the
        // rename has succeeded either way.
        //
        // `Path::parent` answers `Some("")` for a bare file name, and opening
        // `""` fails — which the ignored error would hide, dropping this leg
        // exactly when the CLI was handed a plain `job.json`. That is the
        // invocation the README documents, so the path it produces is the one
        // that most needs the sync. An empty parent means the working directory.
        let parent = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => Path::new("."),
        };

        // Best-effort, and the two ways it fails are not the same thing. Not
        // being able to open a directory at all is the documented platform case
        // — a directory is not a file everywhere — and says nothing about this
        // filesystem, so it stays silent. A directory that *did* open and then
        // failed to sync is a real fault on the one leg the pre-submit write's
        // safety argument rests on, so it is reported rather than swallowed:
        // `save` still returns Ok, and a caller that only reads the return value
        // would otherwise take an unsynced rename for a durable one.
        if let Ok(dir) = fs::File::open(parent)
            && let Err(e) = dir.sync_all()
        {
            eprintln!(
                "dssp-bot: could not sync {} after writing {}: {e} — the checkpoint is saved, \
                 but a power loss could still revert it to the previous one",
                parent.display(),
                self.path.display(),
            );
        }

        Ok(())
    }

    /// `<name>.<pid>.tmp` beside the target: beside, so the rename cannot cross
    /// filesystems, and pid-suffixed, so two runs sharing a directory do not
    /// interleave into one rename target. A fixed temp name is the one way the
    /// "a reader sees the previous file or the new one, never a torn one" claim
    /// above can be false.
    fn temp_path(&self) -> PathBuf {
        let mut name = self.path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{}.tmp", std::process::id()));

        self.path.with_file_name(name)
    }
}

/// Resolve where a batch should checkpoint, without consulting the environment.
///
/// Split out of [`CheckpointStore::for_job`] so the rule is testable on its own —
/// the same split `resolve_against` gets in `main.rs`, and for the same reason:
/// a test that has to check whether `DSSP_CHECKPOINT` happens to be exported
/// asserts nothing on the machines where it is.
///
/// One fixed slot rather than one per job, mirroring the single
/// `dssp.checkpoint` storage key the extension uses: two batches sharing a
/// directory should not silently keep two half-truths about what reached the
/// portal. The override is the escape hatch for exactly that case.
pub fn resolve_path(job_path: &Path, override_path: Option<OsString>) -> PathBuf {
    match override_path {
        Some(path) => PathBuf::from(path),
        None => job_path.with_file_name(DEFAULT_FILE_NAME),
    }
}

fn write_and_sync(path: &Path, json: &str) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(json.as_bytes())?;
    // Before the rename, not after: otherwise a power loss can leave the rename
    // pointing at a file whose contents never reached the disk.
    file.sync_all()
}

impl CheckpointWriter for CheckpointStore {
    /// The engine drops this error, so a failed write has to be reported here or
    /// not at all — same contract, and the same fields, as `writeCheckpoint` in
    /// service-worker.ts.
    fn write(&mut self, checkpoint: &BatchCheckpoint) -> Result<(), String> {
        self.save(checkpoint).inspect_err(|reason| {
            eprintln!(
                "dssp-bot: checkpoint not persisted ({:?}, {} result(s) at risk): {reason}",
                checkpoint.status,
                checkpoint.results.len()
            );
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkpoint::CheckpointStatus;
    use crate::report::{Outcome, TrainingResult};

    /// A unique directory under the system temp dir, so tests never collide.
    struct Scratch {
        dir: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "dssp-bot-store-{}-{name}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&dir).expect("scratch dir");

            Self { dir }
        }

        fn file(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }

        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(&self.dir)
                .expect("read scratch dir")
                .map(|entry| entry.expect("dir entry").file_name().to_string_lossy().into())
                .collect();
            names.sort();

            names
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn checkpoint(status: CheckpointStatus) -> BatchCheckpoint {
        BatchCheckpoint {
            status,
            started_at: "1758285600000".to_string(),
            updated_at: "1758285900000".to_string(),
            total: 3,
            results: vec![
                TrainingResult {
                    trainee_id: "1".to_string(),
                    trainee_name: "Trainee 1".to_string(),
                    outcome: Outcome::Success,
                    attempts: 1,
                    error_code: None,
                    message: Some("ref-1".to_string()),
                },
                TrainingResult {
                    trainee_id: "2".to_string(),
                    trainee_name: "Trainee 2".to_string(),
                    outcome: Outcome::Indeterminate,
                    attempts: 2,
                    error_code: Some("NETWORK".to_string()),
                    message: Some("no confirmation".to_string()),
                },
            ],
            pending: vec!["3".to_string()],
            in_flight: None,
        }
    }

    #[test]
    fn a_missing_file_reads_as_no_checkpoint() {
        let scratch = Scratch::new("missing");

        assert!(CheckpointStore::new(scratch.file("never-written.json"))
            .load()
            .expect("a first run is not an error")
            .is_none());
    }

    /// The whole point of the store: a later process sees what an earlier one
    /// recorded, field for field.
    #[test]
    fn a_saved_checkpoint_loads_back_intact() {
        let scratch = Scratch::new("round-trip");
        let store = CheckpointStore::new(scratch.file("dssp.checkpoint.json"));

        store.save(&checkpoint(CheckpointStatus::Running)).expect("saves");

        let restored = store
            .load()
            .expect("reads")
            .expect("a checkpoint was written");

        assert_eq!(restored.status, CheckpointStatus::Running);
        assert_eq!(restored.started_at, "1758285600000");
        assert_eq!(restored.updated_at, "1758285900000");
        assert_eq!(restored.total, 3);
        assert_eq!(restored.pending, vec!["3"]);
        assert_eq!(restored.results.len(), 2);
        assert_eq!(restored.results[1].outcome, Outcome::Indeterminate);
        assert_eq!(restored.results[1].error_code.as_deref(), Some("NETWORK"));
    }

    /// Recovery marks a live checkpoint interrupted and writes it back, so
    /// overwriting has to be a plain replace rather than an append or a merge.
    #[test]
    fn a_second_save_replaces_the_first() {
        let scratch = Scratch::new("replace");
        let store = CheckpointStore::new(scratch.file("dssp.checkpoint.json"));

        store.save(&checkpoint(CheckpointStatus::Running)).expect("saves");
        store.save(&checkpoint(CheckpointStatus::Finished)).expect("resaves");

        let restored = store.load().expect("reads").expect("still there");

        assert_eq!(restored.status, CheckpointStatus::Finished);
    }

    /// A checkpoint written by a build before `in_flight` existed is still the
    /// only account of what that batch submitted, so the read side has to accept
    /// it — and must still find the record it cannot account for.
    #[test]
    fn a_checkpoint_from_an_older_build_still_loads() {
        let scratch = Scratch::new("older-build");
        let path = scratch.file("dssp.checkpoint.json");
        fs::write(
            &path,
            r#"{
  "status": "running",
  "started_at": "1758285600000",
  "updated_at": "1758285900000",
  "total": 2,
  "results": [
    {
      "trainee_id": "1",
      "trainee_name": "Trainee 1",
      "outcome": "indeterminate",
      "attempts": 1,
      "error_code": "NETWORK"
    }
  ],
  "pending": []
}"#,
        )
        .expect("write an older checkpoint");

        let restored = CheckpointStore::new(&path)
            .load()
            .expect("reads")
            .expect("the file is there");

        assert_eq!(restored.in_flight, None);
        assert!(restored.is_live(), "so the start gate still refuses");
        assert_eq!(restored.unreconciled().indeterminate.len(), 1);
    }

    /// The one thing the typed parse cannot answer, and the reason provenance is
    /// read off the raw JSON: an absent `in_flight` and a `null` one are the same
    /// snapshot, but only the second was written by a build that could record a
    /// submission in flight.
    #[test]
    fn a_file_written_before_in_flight_existed_says_so() {
        let scratch = Scratch::new("provenance");
        let path = scratch.file("dssp.checkpoint.json");
        let store = CheckpointStore::new(&path);

        store.save(&checkpoint(CheckpointStatus::Running)).expect("saves");
        assert!(
            store
                .load_with_provenance()
                .expect("reads")
                .expect("there")
                .tracks_in_flight,
            "a file this build wrote"
        );

        fs::write(
            &path,
            r#"{"status":"running","started_at":"1758285600000","updated_at":"1758285900000",
                "total":1,"results":[],"pending":["1"]}"#,
        )
        .expect("write an older checkpoint");

        let stored = store.load_with_provenance().expect("reads").expect("there");

        assert!(!stored.tracks_in_flight);
        assert_eq!(stored.checkpoint.pending, vec!["1"]);
    }

    /// A corrupt checkpoint is the one thing that must not read as "no
    /// checkpoint" — that would invite a second submission of records that may
    /// already exist.
    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_result() {
        let scratch = Scratch::new("corrupt");
        let path = scratch.file("dssp.checkpoint.json");
        fs::write(&path, r#"{"status":"running","resul"#).expect("write partial json");

        let error = CheckpointStore::new(&path)
            .load()
            .expect_err("must not be read as nothing");

        assert!(error.contains("unreadable checkpoint"), "{error}");
        assert!(error.contains("dssp.checkpoint.json"), "{error}");
    }

    #[test]
    fn a_substring_of_a_checkpoint_does_not_parse() {
        let scratch = Scratch::new("partial");
        let path = scratch.file("dssp.checkpoint.json");
        // The shape a truncated write leaves behind is valid-looking JSON, so
        // the missing fields, not the syntax, have to fail the parse.
        fs::write(&path, r#"{"status":"running","total":3}"#).expect("write");

        assert!(CheckpointStore::new(&path).load().is_err());
    }

    /// The temp file exists only between create and rename; if it survives, a
    /// later `load` would still find the good file, but the directory would
    /// accumulate copies of the most sensitive data in the repo.
    #[test]
    fn a_completed_save_leaves_no_temporary_file_behind() {
        let scratch = Scratch::new("no-temp");
        let store = CheckpointStore::new(scratch.file("dssp.checkpoint.json"));

        store.save(&checkpoint(CheckpointStatus::Running)).expect("saves");

        assert_eq!(scratch.entries(), vec!["dssp.checkpoint.json"]);
    }

    #[test]
    fn a_write_into_a_missing_directory_fails_without_panicking() {
        let scratch = Scratch::new("bad-dir");
        let store = CheckpointStore::new(scratch.file("nowhere").join("dssp.checkpoint.json"));

        let error = store
            .save(&checkpoint(CheckpointStatus::Running))
            .expect_err("no such directory");

        assert!(error.contains("nowhere"), "{error}");
    }

    /// The on-disk shape, field name by field name. Pinned because the file is
    /// read by things that never see these structs — the extension bridge in
    /// Stage 29 above all — and a rename here would silently invalidate it.
    #[test]
    fn the_file_uses_snake_case_field_names() {
        let scratch = Scratch::new("shape");
        let path = scratch.file("dssp.checkpoint.json");
        CheckpointStore::new(&path)
            .save(&checkpoint(CheckpointStatus::Running))
            .expect("saves");

        let text = fs::read_to_string(&path).expect("reads");

        for field in [
            "\"status\"",
            "\"started_at\"",
            "\"updated_at\"",
            "\"total\"",
            "\"results\"",
            "\"pending\"",
            "\"in_flight\"",
            "\"trainee_id\"",
            "\"trainee_name\"",
            "\"outcome\"",
            "\"attempts\"",
            "\"error_code\"",
        ] {
            assert!(text.contains(field), "missing {field} in {text}");
        }

        // The TS original's camelCase must not leak in: a reader keyed on
        // `startedAt` would find nothing and call a real batch a first run.
        for camel in ["\"startedAt\"", "\"traineeId\"", "\"errorCode\"", "\"inFlight\""] {
            assert!(!text.contains(camel), "leaked {camel} in {text}");
        }

        assert!(text.contains("\"running\""), "{text}");
    }

    /// The default location is a fixed name beside the job file, not derived
    /// from it: the checkpoint is per-batch, and `DSSP_CHECKPOINT` is the escape
    /// hatch when two batches need to coexist.
    #[test]
    fn the_default_file_sits_beside_the_job() {
        assert_eq!(
            resolve_path(Path::new("/tmp/batches/job.json"), None),
            Path::new("/tmp/batches/dssp.checkpoint.json")
        );
    }

    /// The override replaces the whole path rather than its file name, so a
    /// checkpoint can be directed outside the job's directory.
    #[test]
    fn the_override_replaces_the_default_entirely() {
        assert_eq!(
            resolve_path(
                Path::new("/tmp/batches/job.json"),
                Some(OsString::from("/var/tmp/elsewhere.json"))
            ),
            Path::new("/var/tmp/elsewhere.json")
        );
    }

    /// Two writers sharing one directory must not share one temp file, or the
    /// loser of the race can rename a half-written file over a good target.
    #[test]
    fn each_run_writes_through_its_own_temporary_file() {
        let scratch = Scratch::new("temp-name");
        let store = CheckpointStore::new(scratch.file("dssp.checkpoint.json"));
        let temp = store.temp_path();
        let name = temp.file_name().unwrap().to_string_lossy().into_owned();

        assert!(name.ends_with(".tmp"), "{name}");
        assert!(name.contains(&std::process::id().to_string()), "{name}");
        assert_eq!(temp.parent(), store.path().parent());
    }
}
