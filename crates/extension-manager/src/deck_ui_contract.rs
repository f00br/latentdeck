use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, ExtensionError, Result};
use crate::model::{CodecCapability, DeckPackManifest};
use crate::schema::parse_strict_json;

const MAX_CONTROLS: usize = 128;
const MAX_SECTIONS: usize = 16;
const MAX_WIDGETS: usize = 128;
const MAX_VISIBILITY_PREDICATES: usize = 8;
const MAX_VISIBILITY_VALUES: usize = 16;
const MAX_JS_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OperatorDescriptor {
    schema_version: String,
    deck_operator_api: String,
    deck_id: String,
    deck_version: String,
    operator_id: String,
    operator_version: String,
    entrypoint: String,
    source_count: u8,
    role_ids: Vec<String>,
    controls: Vec<ControlDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "value_type", rename_all = "snake_case", deny_unknown_fields)]
enum ControlDescriptor {
    Number {
        control_id: String,
        default: f64,
        minimum: f64,
        maximum: f64,
        step: f64,
    },
    Integer {
        control_id: String,
        default: i64,
        minimum: i64,
        maximum: i64,
        step: u64,
    },
    Boolean {
        control_id: String,
        default: bool,
    },
    Enum {
        control_id: String,
        default: String,
        options: Vec<String>,
    },
}

impl ControlDescriptor {
    fn control_id(&self) -> &str {
        match self {
            Self::Number { control_id, .. }
            | Self::Integer { control_id, .. }
            | Self::Boolean { control_id, .. }
            | Self::Enum { control_id, .. } => control_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FaceplateDescriptor {
    schema_version: u16,
    title: String,
    sections: Vec<FaceplateSection>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FaceplateSection {
    section_id: String,
    title: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    region: Option<SectionRegion>,
    #[serde(
        default,
        deserialize_with = "deserialize_present_non_null",
        skip_serializing_if = "Option::is_none"
    )]
    columns: Option<u8>,
    widgets: Vec<FaceplateWidget>,
}

fn deserialize_present_non_null<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SectionRegion {
    Output,
    Actions,
    Controls,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VisibilityPredicate {
    control_id: String,
    one_of: Vec<VisibilityValue>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(untagged)]
enum VisibilityValue {
    Text(String),
    Boolean(bool),
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FaceplateWidget {
    SourcePicker {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        slot_index: u8,
    },
    Slider {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        control_id: String,
        minimum: f64,
        maximum: f64,
        step: f64,
    },
    Number {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        control_id: String,
        minimum: f64,
        maximum: f64,
        step: f64,
    },
    Toggle {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        control_id: String,
    },
    Select {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        control_id: String,
        options: Vec<FaceplateOption>,
    },
    RoleEditor {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        role_ids: Vec<String>,
    },
    Barycentric3 {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        x_control_id: String,
        y_control_id: String,
        vertex_role_ids: [String; 3],
    },
    Transport {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        slot_indices: Vec<u8>,
    },
    Seed {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
    },
    Capture {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
        modes: Vec<CaptureMode>,
    },
    Monitor {
        id: String,
        label: String,
        #[serde(default, deserialize_with = "deserialize_present_non_null")]
        visible_when: Option<Vec<VisibilityPredicate>>,
    },
}

impl FaceplateWidget {
    fn visible_when(&self) -> Option<&[VisibilityPredicate]> {
        match self {
            Self::SourcePicker { visible_when, .. }
            | Self::Slider { visible_when, .. }
            | Self::Number { visible_when, .. }
            | Self::Toggle { visible_when, .. }
            | Self::Select { visible_when, .. }
            | Self::RoleEditor { visible_when, .. }
            | Self::Barycentric3 { visible_when, .. }
            | Self::Transport { visible_when, .. }
            | Self::Seed { visible_when, .. }
            | Self::Capture { visible_when, .. }
            | Self::Monitor { visible_when, .. } => visible_when.as_deref(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FaceplateOption {
    value: String,
    label: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum CaptureMode {
    Snapshot,
    LiveCapture,
}

pub(crate) fn validate_deck_ui_contract(
    manifest: &DeckPackManifest,
    operator_json: &[u8],
    faceplate_json: &[u8],
) -> Result<()> {
    let operator = parse_strict_json::<OperatorDescriptor>(operator_json, "operator.json")?;
    let faceplate = parse_strict_json::<FaceplateDescriptor>(faceplate_json, "faceplate.json")?;
    validate_operator(manifest, &operator)?;
    validate_faceplate(manifest, &operator, &faceplate)
}

fn validate_operator(manifest: &DeckPackManifest, operator: &OperatorDescriptor) -> Result<()> {
    if operator.schema_version != "0.2.0"
        || operator.deck_operator_api != "0.2.0"
        || operator.deck_id != manifest.deck_id
        || operator.deck_version != manifest.deck_version
        || operator.source_count != manifest.signal.slots
        || operator.role_ids
            != manifest
                .signal
                .roles
                .iter()
                .map(|role| role.role_id.clone())
                .collect::<Vec<_>>()
        || operator.entrypoint != manifest.runtime.entrypoint
        || semver::Version::parse(&operator.operator_version).map_or(true, |version| {
            version.to_string() != operator.operator_version
        })
        || !valid_identifier(&operator.operator_id)
        || operator.controls.is_empty()
        || operator.controls.len() > MAX_CONTROLS
    {
        return Err(invalid(
            "operator.json does not match the exact Deck package contract",
        ));
    }
    let mut ids = BTreeSet::new();
    for control in &operator.controls {
        let id = control.control_id();
        if !valid_identifier(id) || !ids.insert(id) || !valid_control(control) {
            return Err(invalid(
                "operator.json contains an invalid or duplicate typed control",
            ));
        }
    }
    Ok(())
}

fn valid_control(control: &ControlDescriptor) -> bool {
    match control {
        ControlDescriptor::Number {
            default,
            minimum,
            maximum,
            step,
            ..
        } => {
            default.is_finite()
                && minimum.is_finite()
                && maximum.is_finite()
                && step.is_finite()
                && minimum < maximum
                && *step > 0.0
                && default >= minimum
                && default <= maximum
        }
        ControlDescriptor::Integer {
            default,
            minimum,
            maximum,
            step,
            ..
        } => {
            minimum < maximum
                && *step > 0
                && default >= minimum
                && default <= maximum
                && *minimum >= -MAX_JS_SAFE_INTEGER
                && *maximum <= MAX_JS_SAFE_INTEGER
                && *default >= -MAX_JS_SAFE_INTEGER
                && *default <= MAX_JS_SAFE_INTEGER
                && *step <= MAX_JS_SAFE_INTEGER as u64
        }
        ControlDescriptor::Boolean { .. } => true,
        ControlDescriptor::Enum {
            default, options, ..
        } => {
            !options.is_empty()
                && options.len() <= 64
                && options.iter().all(|value| valid_identifier(value))
                && options.iter().collect::<BTreeSet<_>>().len() == options.len()
                && options.contains(default)
        }
    }
}

#[allow(clippy::float_cmp, clippy::too_many_lines)]
fn validate_faceplate(
    manifest: &DeckPackManifest,
    operator: &OperatorDescriptor,
    faceplate: &FaceplateDescriptor,
) -> Result<()> {
    if !matches!(faceplate.schema_version, 1 | 2)
        || !valid_text(&faceplate.title)
        || faceplate.sections.is_empty()
        || faceplate.sections.len() > MAX_SECTIONS
    {
        return Err(invalid(
            "faceplate.json has an unsupported schema or exceeds its limits",
        ));
    }
    let controls = operator
        .controls
        .iter()
        .map(|control| (control.control_id(), control))
        .collect::<BTreeMap<_, _>>();
    let role_ids = manifest
        .signal
        .roles
        .iter()
        .map(|role| role.role_id.as_str())
        .collect::<BTreeSet<_>>();
    let expected_slots = (0..manifest.signal.slots).collect::<BTreeSet<_>>();
    let mut section_ids = BTreeSet::new();
    let mut widget_ids = BTreeSet::new();
    let mut source_slots = BTreeSet::new();
    let mut bound_controls = BTreeSet::new();
    let mut role_editor_count = 0;
    let mut transport_count = 0;
    let mut seed_count = 0;
    let mut capture_count = 0;
    let mut monitor_count = 0;
    let mut output_region_count = 0;
    let mut widget_count = 0;
    for section in &faceplate.sections {
        if !valid_identifier(&section.section_id)
            || !section_ids.insert(section.section_id.as_str())
            || !valid_text(&section.title)
        {
            return Err(invalid(
                "faceplate.json contains an invalid or duplicate section",
            ));
        }
        if faceplate.schema_version == 1 {
            if section.region.is_some() || section.columns.is_some() {
                return Err(invalid(
                    "faceplate schema v1 cannot declare schema-v2 layout fields",
                ));
            }
        } else {
            let Some(region) = section.region else {
                return Err(invalid(
                    "faceplate schema v2 requires a region and bounded column count per section",
                ));
            };
            if !matches!(section.columns, Some(1..=4)) {
                return Err(invalid(
                    "faceplate schema v2 requires a region and bounded column count per section",
                ));
            }
            if region == SectionRegion::Output {
                output_region_count += 1;
            }
        }
        widget_count += section.widgets.len();
        if widget_count > MAX_WIDGETS {
            return Err(invalid(
                "faceplate.json exceeds the host-rendered widget limit",
            ));
        }
        for widget in &section.widgets {
            if faceplate.schema_version == 1 {
                if widget.visible_when().is_some() {
                    return Err(invalid(
                        "faceplate schema v1 cannot declare schema-v2 visibility predicates",
                    ));
                }
            } else {
                validate_visibility(&controls, widget.visible_when())?;
                let valid_region = match widget {
                    FaceplateWidget::Monitor { .. } => {
                        section.region == Some(SectionRegion::Output)
                    }
                    FaceplateWidget::Capture { .. } => {
                        section.region == Some(SectionRegion::Actions)
                    }
                    FaceplateWidget::SourcePicker { .. }
                    | FaceplateWidget::Slider { .. }
                    | FaceplateWidget::Number { .. }
                    | FaceplateWidget::Toggle { .. }
                    | FaceplateWidget::Select { .. }
                    | FaceplateWidget::RoleEditor { .. }
                    | FaceplateWidget::Barycentric3 { .. }
                    | FaceplateWidget::Transport { .. }
                    | FaceplateWidget::Seed { .. } => {
                        section.region == Some(SectionRegion::Controls)
                    }
                };
                if !valid_region {
                    return Err(invalid(
                        "a schema-v2 faceplate widget occupies an invalid layout region",
                    ));
                }
            }
            let (id, label) = widget_identity(widget);
            if !valid_identifier(id) || !widget_ids.insert(id) || !valid_text(label) {
                return Err(invalid(
                    "faceplate.json contains an invalid or duplicate widget",
                ));
            }
            match widget {
                FaceplateWidget::SourcePicker { slot_index, .. } => {
                    if !expected_slots.contains(slot_index) || !source_slots.insert(*slot_index) {
                        return Err(invalid(
                            "a source picker references an absent or duplicate physical slot",
                        ));
                    }
                }
                FaceplateWidget::Slider {
                    control_id,
                    minimum,
                    maximum,
                    step,
                    ..
                } => validate_numeric_widget(
                    &controls,
                    &mut bound_controls,
                    control_id,
                    *minimum,
                    *maximum,
                    *step,
                    false,
                )?,
                FaceplateWidget::Number {
                    control_id,
                    minimum,
                    maximum,
                    step,
                    ..
                } => validate_numeric_widget(
                    &controls,
                    &mut bound_controls,
                    control_id,
                    *minimum,
                    *maximum,
                    *step,
                    true,
                )?,
                FaceplateWidget::Toggle { control_id, .. } => {
                    if !matches!(
                        controls.get(control_id.as_str()),
                        Some(ControlDescriptor::Boolean { .. })
                    ) || !bound_controls.insert(control_id.as_str())
                    {
                        return Err(invalid(
                            "a toggle does not match one unique boolean control",
                        ));
                    }
                }
                FaceplateWidget::Select {
                    control_id,
                    options,
                    ..
                } => {
                    let option_values = options
                        .iter()
                        .map(|option| option.value.as_str())
                        .collect::<BTreeSet<_>>();
                    if options.is_empty()
                        || options.len() > 64
                        || option_values.len() != options.len()
                        || options.iter().any(|option| {
                            !valid_identifier(&option.value) || !valid_text(&option.label)
                        })
                        || !matches!(
                            controls.get(control_id.as_str()),
                            Some(ControlDescriptor::Enum { options: declared, .. })
                                if declared.iter().map(String::as_str).collect::<BTreeSet<_>>() == option_values
                        )
                        || !bound_controls.insert(control_id.as_str())
                    {
                        return Err(invalid("a select does not match one unique enum control"));
                    }
                }
                FaceplateWidget::RoleEditor {
                    role_ids: roles, ..
                } => {
                    role_editor_count += 1;
                    if roles.len() != role_ids.len()
                        || roles.iter().map(String::as_str).collect::<BTreeSet<_>>() != role_ids
                    {
                        return Err(invalid(
                            "the role editor does not cover the Deck role contract",
                        ));
                    }
                }
                FaceplateWidget::Barycentric3 {
                    x_control_id,
                    y_control_id,
                    vertex_role_ids,
                    ..
                } => {
                    if vertex_role_ids
                        .iter()
                        .map(String::as_str)
                        .collect::<BTreeSet<_>>()
                        .len()
                        != 3
                        || vertex_role_ids
                            .iter()
                            .any(|role| !role_ids.contains(role.as_str()))
                    {
                        return Err(invalid(
                            "the barycentric widget does not reference three distinct Deck roles",
                        ));
                    }
                    let mut defaults = Vec::with_capacity(2);
                    for control_id in [x_control_id, y_control_id] {
                        let Some(ControlDescriptor::Number {
                            default,
                            minimum,
                            maximum,
                            ..
                        }) = controls.get(control_id.as_str())
                        else {
                            return Err(invalid(
                                "the barycentric widget must bind two normalized number controls",
                            ));
                        };
                        if *minimum != 0.0
                            || *maximum != 1.0
                            || !bound_controls.insert(control_id.as_str())
                        {
                            return Err(invalid(
                                "the barycentric widget must bind two unique normalized number controls",
                            ));
                        }
                        defaults.push(*default);
                    }
                    let [x, y] = defaults.as_slice() else {
                        unreachable!("two barycentric controls are always collected")
                    };
                    if *x < 0.5 * *y - 1e-12 || *x > 1.0 - 0.5 * *y + 1e-12 {
                        return Err(invalid(
                            "the barycentric control defaults lie outside the declared triangle",
                        ));
                    }
                }
                FaceplateWidget::Transport { slot_indices, .. } => {
                    transport_count += 1;
                    if slot_indices.iter().copied().collect::<BTreeSet<_>>() != expected_slots
                        || slot_indices.len() != expected_slots.len()
                    {
                        return Err(invalid(
                            "the transport widget does not cover every physical slot",
                        ));
                    }
                }
                FaceplateWidget::Seed { .. } => seed_count += 1,
                FaceplateWidget::Capture { modes, .. } => {
                    capture_count += 1;
                    if modes.is_empty()
                        || modes.len() > 2
                        || modes.iter().copied().collect::<BTreeSet<_>>().len() != modes.len()
                        || modes.iter().any(|mode| match mode {
                            CaptureMode::Snapshot => !manifest
                                .signal
                                .required_capabilities
                                .contains(&CodecCapability::SnapshotCapture),
                            CaptureMode::LiveCapture => !manifest
                                .signal
                                .required_capabilities
                                .contains(&CodecCapability::LiveCapture),
                        })
                    {
                        return Err(invalid(
                            "the capture widget exceeds the declared Deck capabilities",
                        ));
                    }
                }
                FaceplateWidget::Monitor { .. } => monitor_count += 1,
            }
        }
    }
    if source_slots != expected_slots
        || bound_controls.len() != controls.len()
        || bound_controls.iter().any(|id| !controls.contains_key(*id))
        || role_editor_count != 1
        || transport_count != 1
        || seed_count != 1
        || capture_count > 1
        || monitor_count != 1
        || (faceplate.schema_version == 2 && output_region_count != 1)
    {
        return Err(invalid(
            "faceplate.json does not expose the complete closed Deck contract exactly once",
        ));
    }
    Ok(())
}

fn validate_visibility(
    controls: &BTreeMap<&str, &ControlDescriptor>,
    predicates: Option<&[VisibilityPredicate]>,
) -> Result<()> {
    let Some(predicates) = predicates else {
        return Ok(());
    };
    if predicates.is_empty() || predicates.len() > MAX_VISIBILITY_PREDICATES {
        return Err(invalid(
            "a visibility predicate list is empty or exceeds its limit",
        ));
    }
    for predicate in predicates {
        if !valid_identifier(&predicate.control_id)
            || predicate.one_of.is_empty()
            || predicate.one_of.len() > MAX_VISIBILITY_VALUES
            || predicate.one_of.iter().collect::<BTreeSet<_>>().len() != predicate.one_of.len()
            || predicate.one_of.iter().any(
                |value| matches!(value, VisibilityValue::Text(text) if !valid_identifier(text)),
            )
        {
            return Err(invalid(
                "a visibility predicate is invalid, duplicated, unsafe, or exceeds its limits",
            ));
        }
        let Some(control) = controls.get(predicate.control_id.as_str()) else {
            return Err(invalid(
                "a visibility predicate references an absent operator control",
            ));
        };
        let matches_control = match control {
            ControlDescriptor::Enum { options, .. } => predicate.one_of.iter().all(
                |value| matches!(value, VisibilityValue::Text(text) if options.contains(text)),
            ),
            ControlDescriptor::Boolean { .. } => predicate
                .one_of
                .iter()
                .all(|value| matches!(value, VisibilityValue::Boolean(_))),
            ControlDescriptor::Number { .. } | ControlDescriptor::Integer { .. } => false,
        };
        if !matches_control {
            return Err(invalid(
                "a visibility predicate must match one enum or boolean control exactly",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::cast_precision_loss, clippy::float_cmp)]
fn validate_numeric_widget<'a>(
    controls: &BTreeMap<&'a str, &'a ControlDescriptor>,
    bound_controls: &mut BTreeSet<&'a str>,
    control_id: &'a str,
    minimum: f64,
    maximum: f64,
    step: f64,
    number_widget: bool,
) -> Result<()> {
    let Some(control) = controls.get(control_id) else {
        return Err(invalid(
            "a numeric widget references an absent operator control",
        ));
    };
    let matches = match control {
        ControlDescriptor::Number {
            minimum: declared_minimum,
            maximum: declared_maximum,
            step: declared_step,
            ..
        } => minimum == *declared_minimum && maximum == *declared_maximum && step == *declared_step,
        ControlDescriptor::Integer {
            minimum: declared_minimum,
            maximum: declared_maximum,
            step: declared_step,
            ..
        } => {
            number_widget
                && minimum == *declared_minimum as f64
                && maximum == *declared_maximum as f64
                && step == *declared_step as f64
        }
        ControlDescriptor::Boolean { .. } | ControlDescriptor::Enum { .. } => false,
    };
    if !matches || !bound_controls.insert(control_id) {
        return Err(invalid(
            "a numeric widget does not match one unique typed operator control",
        ));
    }
    Ok(())
}

fn widget_identity(widget: &FaceplateWidget) -> (&str, &str) {
    match widget {
        FaceplateWidget::SourcePicker { id, label, .. }
        | FaceplateWidget::Slider { id, label, .. }
        | FaceplateWidget::Number { id, label, .. }
        | FaceplateWidget::Toggle { id, label, .. }
        | FaceplateWidget::Select { id, label, .. }
        | FaceplateWidget::RoleEditor { id, label, .. }
        | FaceplateWidget::Barycentric3 { id, label, .. }
        | FaceplateWidget::Transport { id, label, .. }
        | FaceplateWidget::Seed { id, label, .. }
        | FaceplateWidget::Capture { id, label, .. }
        | FaceplateWidget::Monitor { id, label, .. } => (id, label),
    }
}

fn valid_identifier(value: &str) -> bool {
    if value.is_empty() || value.len() > 128 || !value.is_ascii() {
        return false;
    }
    value.split(['.', '_', '-']).all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

fn valid_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn invalid(detail: &'static str) -> ExtensionError {
    ExtensionError::new(ErrorCode::ManifestInvalid, detail)
}
