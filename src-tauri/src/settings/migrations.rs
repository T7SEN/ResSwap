//! Schema migrations: one step per version, run in version order on the raw JSON before the
//! file is deserialised (REQ-SET-03, D-004). settings.json and journal.json share the runner;
//! each file keeps its own list of steps and the place of its version field.

#![forbid(unsafe_code)]

use serde_json::Value;

/// One migration: turns a file of version `from` into version `from + 1`, the version field
/// included.
#[derive(Clone, Copy, Debug)]
pub struct Step {
    pub from: u32,
    pub run: fn(Value) -> Result<Value, MigrationError>,
}

/// The settings steps, in order. Version 1 is the first shape ResSwap writes, so the list is
/// empty until a field changes; each new step comes with its fixture test.
pub const SETTINGS_STEPS: &[Step] = &[];

/// Why a file could not be brought to the version this build reads.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// The version field is missing, or is not a whole number from 1 up.
    #[error("the schema version is missing or not a whole number from 1 up")]
    NoVersion,
    /// A newer build wrote the file: it is never migrated down.
    #[error("schema version {found} is newer than version {known}, which this build reads")]
    Newer { found: u64, known: u32 },
    /// No step leads on from this version.
    #[error("no migration leads on from schema version {from}")]
    MissingStep { from: u32 },
    /// A step refused the file, or left another version than the next one.
    #[error("the migration from schema version {from} failed: {reason}")]
    StepFailed { from: u32, reason: String },
}

/// The version at `pointer` in `raw` (a JSON pointer such as "/schemaVersion").
pub fn version_at(raw: &Value, pointer: &str) -> Result<u64, MigrationError> {
    match raw.pointer(pointer).and_then(Value::as_u64) {
        Some(version) if version >= 1 => Ok(version),
        _ => Err(MigrationError::NoVersion),
    }
}

/// Runs every step from the file's version up to `current`, in order, each on the raw JSON.
/// A file already at `current` comes back unchanged.
pub fn run(
    raw: Value,
    pointer: &str,
    steps: &[Step],
    current: u32,
) -> Result<Value, MigrationError> {
    let found = version_at(&raw, pointer)?;
    let Ok(mut version) = u32::try_from(found) else {
        return Err(MigrationError::Newer {
            found,
            known: current,
        });
    };
    if version > current {
        return Err(MigrationError::Newer {
            found,
            known: current,
        });
    }
    let mut value = raw;
    while version < current {
        let step = steps
            .iter()
            .find(|step| step.from == version)
            .ok_or(MigrationError::MissingStep { from: version })?;
        value = (step.run)(value)?;
        let next = version_at(&value, pointer)?;
        if next != u64::from(version) + 1 {
            return Err(MigrationError::StepFailed {
                from: version,
                reason: format!("it left schema version {next}"),
            });
        }
        version += 1;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const POINTER: &str = "/settings/schemaVersion";

    fn set_version(value: &mut Value, version: u32) {
        if let Some(field) = value.pointer_mut(POINTER) {
            *field = json!(version);
        }
    }

    /// Version 1 to 2 in a test: adds a field and records that it ran.
    fn one_to_two(mut value: Value) -> Result<Value, MigrationError> {
        value["trace"] = json!(["1 to 2"]);
        set_version(&mut value, 2);
        Ok(value)
    }

    /// Version 2 to 3 in a test: needs the field the step before added.
    fn two_to_three(mut value: Value) -> Result<Value, MigrationError> {
        let trace = value["trace"]
            .as_array_mut()
            .ok_or(MigrationError::StepFailed {
                from: 2,
                reason: String::from("no trace"),
            })?;
        trace.push(json!("2 to 3"));
        set_version(&mut value, 3);
        Ok(value)
    }

    fn keeps_version(value: Value) -> Result<Value, MigrationError> {
        Ok(value)
    }

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../../tests/fixtures/settings-v1-full.json")).unwrap()
    }

    /// The runner on the version 1 fixture with two steps written for the test: each runs
    /// once, in version order, whatever order the list holds them in.
    #[test]
    fn req_set_03_steps_run_in_version_order() {
        let steps = [
            Step {
                from: 2,
                run: two_to_three,
            },
            Step {
                from: 1,
                run: one_to_two,
            },
        ];
        let migrated = run(fixture(), POINTER, &steps, 3).unwrap();
        assert_eq!(migrated["trace"], json!(["1 to 2", "2 to 3"]));
        assert_eq!(version_at(&migrated, POINTER), Ok(3));
        // Everything else is as the fixture had it.
        let mut expected = fixture();
        expected["trace"] = json!(["1 to 2", "2 to 3"]);
        set_version(&mut expected, 3);
        assert_eq!(migrated, expected);
    }

    /// Version 1 is current today, so the settings list is empty and a version 1 file passes
    /// through unchanged.
    #[test]
    fn req_set_03_current_version_passes_through() {
        assert!(SETTINGS_STEPS.is_empty());
        let raw = fixture();
        assert_eq!(run(raw.clone(), POINTER, SETTINGS_STEPS, 1), Ok(raw));
    }

    #[test]
    fn req_set_03_refusals() {
        let steps = [Step {
            from: 1,
            run: keeps_version,
        }];
        assert_eq!(
            run(fixture(), POINTER, &steps, 2),
            Err(MigrationError::StepFailed {
                from: 1,
                reason: String::from("it left schema version 1"),
            })
        );
        assert_eq!(
            run(fixture(), POINTER, &[], 2),
            Err(MigrationError::MissingStep { from: 1 })
        );
        let mut newer = fixture();
        newer["settings"]["schemaVersion"] = json!(7);
        assert_eq!(
            run(newer, POINTER, &[], 1),
            Err(MigrationError::Newer { found: 7, known: 1 })
        );
        let mut huge = fixture();
        huge["settings"]["schemaVersion"] = json!(u64::MAX);
        assert_eq!(
            run(huge, POINTER, &[], 1),
            Err(MigrationError::Newer {
                found: u64::MAX,
                known: 1
            })
        );
        for version in [json!(0), json!(-1), json!(1.5), json!("1"), Value::Null] {
            let mut damaged = fixture();
            damaged["settings"]["schemaVersion"] = version.clone();
            assert_eq!(
                run(damaged, POINTER, &[], 1),
                Err(MigrationError::NoVersion),
                "{version}"
            );
        }
        assert_eq!(
            run(json!({ "settings": {} }), POINTER, &[], 1),
            Err(MigrationError::NoVersion)
        );
    }
}
