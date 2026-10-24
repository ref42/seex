use std::collections::HashSet;

use serde_json::Value;

use crate::easyeda::ComponentData;
use crate::lceda::SearchItem;
use crate::lcsc::LcscProduct;
use crate::util::{nested_string, value_to_string};

const SEEX_AUTHOR: &str = "ref42";
const SEEX_PROJECT_URL: &str = "https://github.com/ref42/seex";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedMetadata {
    pub lcsc_id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub designator: Option<String>,
    pub comment: Option<String>,
    pub manufacturer: Option<String>,
    pub manufacturer_part: Option<String>,
    pub supplier: Option<String>,
    pub supplier_part: Option<String>,
    pub lcsc_part: Option<String>,
    pub footprint: Option<String>,
    pub datasheet: Option<String>,
    pub category: Option<String>,
    pub parameters: Vec<MetadataParameter>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataParameter {
    pub name: String,
    pub value: String,
}

impl ResolvedMetadata {
    pub fn from_lceda_search_item(
        item: &SearchItem,
        symbol_data: &Value,
        footprint_model_name: Option<&str>,
        preferred_product: Option<&LcscProduct>,
    ) -> Self {
        let attributes = item.raw.get("attributes").and_then(Value::as_object);
        let lcsc_id = item.lcsc_id();
        let footprint = clean_option(footprint_model_name.map(ToOwned::to_owned)).or_else(|| {
            first_non_empty([
                nested_string(&item.raw, &["footprint", "display_title"]),
                find_attribute_value_case_insensitive(attributes, "Supplier Footprint"),
            ])
        });

        let mut metadata = Self {
            lcsc_id: lcsc_id.clone(),
            title: first_non_empty([
                find_attribute_value_case_insensitive(attributes, "Manufacturer Part"),
                find_attribute_value_case_insensitive(attributes, "Value"),
                find_attribute_value_case_insensitive(attributes, "LCSC Part Name"),
                clean_value(item.display_name()),
            ]),
            description: first_non_empty([
                nested_string(&item.raw, &["description"]),
                nested_string(symbol_data, &["result", "description"]),
                find_attribute_value_case_insensitive(attributes, "LCSC Part Name"),
                find_attribute_value_case_insensitive(attributes, "Manufacturer Part"),
            ]),
            designator: find_attribute_value_case_insensitive(attributes, "Designator"),
            comment: resolve_schlib_comment(item),
            manufacturer: clean_value(&item.manufacturer).or_else(|| {
                first_non_empty([
                    find_attribute_value_case_insensitive(attributes, "Manufacturer"),
                    find_attribute_value_case_insensitive(attributes, "BOM_Manufacturer"),
                ])
            }),
            manufacturer_part: find_attribute_value_case_insensitive(
                attributes,
                "Manufacturer Part",
            ),
            supplier: find_attribute_value_case_insensitive(attributes, "Supplier")
                .or_else(|| lcsc_id.as_ref().map(|_| "LCSC".to_string())),
            supplier_part: find_attribute_value_case_insensitive(attributes, "Supplier Part")
                .or_else(|| lcsc_id.clone()),
            lcsc_part: find_attribute_value_case_insensitive(attributes, "LCSC Part")
                .or_else(|| lcsc_id.clone()),
            footprint,
            datasheet: find_attribute_value_case_insensitive(attributes, "Datasheet"),
            category: find_attribute_value_case_insensitive(attributes, "Category"),
            parameters: Vec::new(),
        };

        if let Some(product) = preferred_product {
            metadata.apply_lcsc_product(product);
        }

        metadata.parameters = build_lceda_parameters(
            item,
            preferred_product,
            metadata.lcsc_id.as_deref(),
            &metadata.footprint,
        );
        metadata
    }

    pub fn from_component_data(
        component_data: &ComponentData,
        preferred_product: Option<&LcscProduct>,
    ) -> Self {
        let mut metadata = Self {
            lcsc_id: clean_value(&component_data.lcsc_id),
            title: clean_value(&component_data.title),
            description: clean_value(&component_data.description),
            manufacturer: clean_value(&component_data.manufacturer),
            manufacturer_part: clean_value(&component_data.title),
            supplier: clean_value("LCSC"),
            supplier_part: clean_value(&component_data.lcsc_id),
            lcsc_part: clean_value(&component_data.lcsc_id),
            datasheet: clean_value(&component_data.datasheet),
            parameters: build_project_parameters(),
            ..Self::default()
        };

        if let Some(product) = preferred_product {
            metadata.apply_lcsc_product(product);
        }

        metadata
    }

    pub fn apply_to_component_data(&self, component_data: &mut ComponentData) {
        assign_if_present(&mut component_data.title, self.title.as_deref());
        assign_if_present(&mut component_data.description, self.description.as_deref());
        assign_if_present(
            &mut component_data.manufacturer,
            self.manufacturer.as_deref(),
        );
        assign_if_present(&mut component_data.datasheet, self.datasheet.as_deref());
        assign_if_present(&mut component_data.lcsc_id, self.lcsc_id.as_deref());
    }

    fn apply_lcsc_product(&mut self, product: &LcscProduct) {
        if let Some(sku) = clean_value(&product.sku) {
            if self.lcsc_id.is_none() {
                self.lcsc_id = Some(sku.clone());
            }
            self.supplier = Some("LCSC".to_string());
            self.supplier_part = Some(sku.clone());
            self.lcsc_part = Some(sku);
        }
        if let Some(mpn) = clean_option(product.mpn.clone()) {
            self.title = Some(mpn.clone());
            self.comment = Some(mpn.clone());
            self.manufacturer_part = Some(mpn);
        }
        if let Some(manufacturer) = clean_option(product.manufacturer.clone()) {
            self.manufacturer = Some(manufacturer);
        }
        if let Some(description) = clean_option(product.description.clone()) {
            self.description = Some(description);
        }
        if let Some(category) = clean_option(product.category.clone()) {
            self.category = Some(category);
        }
        if let Some(datasheet_url) = clean_option(product.datasheet_url.clone()) {
            self.datasheet = Some(datasheet_url);
        }
    }
}

fn build_lceda_parameters(
    item: &SearchItem,
    preferred_product: Option<&LcscProduct>,
    lcsc_id: Option<&str>,
    footprint: &Option<String>,
) -> Vec<MetadataParameter> {
    let mut builder = ParameterBuilder::default();
    builder.push("SEEX_AUTHOR", SEEX_AUTHOR);
    builder.push("SEEX_PROJECT_URL", SEEX_PROJECT_URL);
    if let Some(component_id) = lcsc_id {
        builder.push("SEEX_COMPONENT_ID", component_id);
    }
    if let Some(footprint_name) = footprint.as_deref() {
        builder.push("Footprint", footprint_name);
    }

    if let Some(product) = preferred_product {
        push_lcsc_product_parameters(&mut builder, product);
    }

    if let Some(attributes) = item.raw.get("attributes").and_then(Value::as_object) {
        for (name, value) in attributes {
            if should_skip_schlib_parameter(name) {
                continue;
            }
            let Some(value) = value_to_string(value) else {
                continue;
            };
            builder.push(name, value);
        }
    }

    builder.parameters
}

fn build_project_parameters() -> Vec<MetadataParameter> {
    let mut builder = ParameterBuilder::default();
    builder.push("SEEX_AUTHOR", SEEX_AUTHOR);
    builder.push("SEEX_PROJECT_URL", SEEX_PROJECT_URL);
    builder.parameters
}

fn push_lcsc_product_parameters(builder: &mut ParameterBuilder, product: &LcscProduct) {
    builder.push("Supplier", "LCSC");
    builder.push("Supplier Part", &product.sku);
    builder.push("LCSC Part", &product.sku);
    if let Some(mpn) = product.mpn.as_deref() {
        builder.push("Manufacturer Part", mpn);
    }
    if let Some(manufacturer) = product.manufacturer.as_deref() {
        builder.push("Manufacturer", manufacturer);
    }
    if let Some(description) = product.description.as_deref() {
        builder.push("LCSC Part Name", description);
    }
    if let Some(category) = product.category.as_deref() {
        builder.push("Category", category);
    }
    if let Some(datasheet_url) = product.datasheet_url.as_deref() {
        builder.push("Datasheet", datasheet_url);
    }
    for property in &product.properties {
        builder.push(&property.name, &property.value);
    }
}

#[derive(Default)]
struct ParameterBuilder {
    parameters: Vec<MetadataParameter>,
    seen_names: HashSet<String>,
}

impl ParameterBuilder {
    fn push(&mut self, name: impl AsRef<str>, value: impl AsRef<str>) {
        let Some(name) = clean_value(name.as_ref()) else {
            return;
        };
        let Some(value) = clean_value(value.as_ref()) else {
            return;
        };
        let normalized = name.to_ascii_lowercase();
        if !self.seen_names.insert(normalized) {
            return;
        }
        self.parameters.push(MetadataParameter { name, value });
    }
}

fn resolve_schlib_comment(item: &SearchItem) -> Option<String> {
    let attributes = item.raw.get("attributes").and_then(Value::as_object);

    if let Some(name) = find_attribute_value_case_insensitive(attributes, "Name") {
        if let Some(resolved) = resolve_attribute_formula(&name, attributes) {
            return Some(resolved);
        }
        if extract_formula_field(&name).is_none() {
            return Some(name);
        }
    }

    first_non_empty([
        find_attribute_value_case_insensitive(attributes, "Manufacturer Part"),
        find_attribute_value_case_insensitive(attributes, "Value"),
        find_attribute_value_case_insensitive(attributes, "LCSC Part Name"),
        clean_value(item.display_name()),
    ])
}

fn resolve_attribute_formula(
    text: &str,
    attributes: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    let field = extract_formula_field(text)?;
    find_attribute_value_case_insensitive(attributes, &field)
}

fn extract_formula_field(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let rhs = trimmed.strip_prefix('=')?.trim();
    let field = rhs.strip_prefix('{')?.strip_suffix('}')?.trim();
    clean_value(field)
}

fn find_attribute_value_case_insensitive(
    attributes: Option<&serde_json::Map<String, Value>>,
    name: &str,
) -> Option<String> {
    let attributes = attributes?;
    attributes
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value_to_string(value))
        .and_then(|value| clean_value(&value))
}

fn first_non_empty(candidates: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    candidates
        .into_iter()
        .flatten()
        .find_map(|value| clean_value(&value))
}

fn clean_option(value: Option<String>) -> Option<String> {
    value.and_then(|value| clean_value(&value))
}

fn clean_value(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed == "-" {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn assign_if_present(target: &mut String, value: Option<&str>) {
    if let Some(value) = value.and_then(clean_value) {
        *target = value;
    }
}

fn should_skip_schlib_parameter(name: &str) -> bool {
    const SKIP: [&str; 9] = [
        "Add into BOM",
        "Convert to PCB",
        "Symbol",
        "Designator",
        "Footprint",
        "3D Model",
        "3D Model Title",
        "3D Model Transform",
        "Name",
    ];
    SKIP.iter().any(|item| item.eq_ignore_ascii_case(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::easyeda::ComponentData;
    use crate::lcsc::{LcscProduct, LcscProperty};
    use serde_json::json;

    fn parameter_value<'a>(metadata: &'a ResolvedMetadata, name: &str) -> Option<&'a str> {
        metadata
            .parameters
            .iter()
            .find(|parameter| parameter.name.eq_ignore_ascii_case(name))
            .map(|parameter| parameter.value.as_str())
    }

    #[test]
    fn lceda_metadata_uses_source_when_product_is_missing() {
        let item = SearchItem {
            index: 0,
            display_title: "RP2040".to_string(),
            title: String::new(),
            manufacturer: "Raspberry Pi".to_string(),
            model_uuid: None,
            raw: json!({
                "product_code": "C2040",
                "attributes": {
                    "Name": "={Manufacturer Part}",
                    "Manufacturer Part": "RP2040",
                    "LCSC Part Name": "MCU",
                    "Supplier Footprint": "QFN-56",
                    "Voltage": "3.3V"
                }
            }),
        };

        let metadata = ResolvedMetadata::from_lceda_search_item(
            &item,
            &json!({"result": {"description": "Source description"}}),
            None,
            None,
        );

        assert_eq!(metadata.comment.as_deref(), Some("RP2040"));
        assert_eq!(metadata.description.as_deref(), Some("Source description"));
        assert_eq!(metadata.manufacturer.as_deref(), Some("Raspberry Pi"));
        assert_eq!(metadata.footprint.as_deref(), Some("QFN-56"));
        assert_eq!(parameter_value(&metadata, "SEEX_AUTHOR"), Some("ref42"));
        assert_eq!(
            parameter_value(&metadata, "SEEX_PROJECT_URL"),
            Some("https://github.com/ref42/seex")
        );
        assert_eq!(
            parameter_value(&metadata, "SEEX_COMPONENT_ID"),
            Some("C2040")
        );
        assert_eq!(parameter_value(&metadata, "Voltage"), Some("3.3V"));
    }

    #[test]
    fn lceda_metadata_overlays_product_without_losing_source_fallbacks() {
        let item = SearchItem {
            index: 0,
            display_title: "CN title".to_string(),
            title: String::new(),
            manufacturer: "Source Manufacturer".to_string(),
            model_uuid: None,
            raw: json!({
                "product_code": "C1",
                "attributes": {
                    "Manufacturer Part": "SOURCE-MPN",
                    "LCSC Part Name": "Source description",
                    "Supplier Footprint": "SOT-23",
                    "Voltage": "5V"
                }
            }),
        };
        let product = LcscProduct {
            sku: "C1".to_string(),
            mpn: Some("EN-MPN".to_string()),
            manufacturer: None,
            description: None,
            category: Some("Transistors".to_string()),
            datasheet_url: Some("https://example.test/ds.pdf".to_string()),
            properties: vec![LcscProperty {
                name: "Package".to_string(),
                value: "SOT-23".to_string(),
            }],
        };

        let metadata = ResolvedMetadata::from_lceda_search_item(
            &item,
            &json!({}),
            Some("SOT-23"),
            Some(&product),
        );

        assert_eq!(metadata.comment.as_deref(), Some("EN-MPN"));
        assert_eq!(metadata.description.as_deref(), Some("Source description"));
        assert_eq!(
            metadata.manufacturer.as_deref(),
            Some("Source Manufacturer")
        );
        assert_eq!(
            parameter_value(&metadata, "Manufacturer Part"),
            Some("EN-MPN")
        );
        assert_eq!(parameter_value(&metadata, "Voltage"), Some("5V"));
        assert_eq!(parameter_value(&metadata, "Package"), Some("SOT-23"));
    }

    #[test]
    fn component_metadata_keeps_base_values_when_product_missing() {
        let component_data = component_data();
        let metadata = ResolvedMetadata::from_component_data(&component_data, None);

        assert_eq!(metadata.title.as_deref(), Some("CN-TITLE"));
        assert_eq!(metadata.description.as_deref(), Some("CN description"));
        assert_eq!(metadata.manufacturer.as_deref(), Some("CN Manufacturer"));
        assert_eq!(
            metadata.datasheet.as_deref(),
            Some("https://source.test/ds")
        );
    }

    #[test]
    fn component_metadata_applies_partial_product_without_blanking_fields() {
        let mut component_data = component_data();
        let product = LcscProduct {
            sku: "C123".to_string(),
            mpn: Some("EN-MPN".to_string()),
            manufacturer: None,
            description: Some("English description".to_string()),
            category: None,
            datasheet_url: None,
            properties: Vec::new(),
        };
        let metadata = ResolvedMetadata::from_component_data(&component_data, Some(&product));
        metadata.apply_to_component_data(&mut component_data);

        assert_eq!(component_data.title, "EN-MPN");
        assert_eq!(component_data.description, "English description");
        assert_eq!(component_data.manufacturer, "CN Manufacturer");
        assert_eq!(component_data.datasheet, "https://source.test/ds");
    }

    #[test]
    fn resolves_manufacturer_part_formula_comment() {
        let item = SearchItem {
            index: 0,
            display_title: "RP2040".to_string(),
            title: String::new(),
            manufacturer: "Raspberry Pi".to_string(),
            model_uuid: None,
            raw: json!({
                "attributes": {
                    "Name": "={Manufacturer Part}",
                    "Manufacturer Part": "RP2040"
                }
            }),
        };

        assert_eq!(resolve_schlib_comment(&item).as_deref(), Some("RP2040"));
    }

    fn component_data() -> ComponentData {
        ComponentData {
            lcsc_id: "C123".to_string(),
            title: "CN-TITLE".to_string(),
            designator: "U?".to_string(),
            description: "CN description".to_string(),
            data_str: Vec::new(),
            bbox_x: 0.0,
            bbox_y: 0.0,
            symbol_parts: Vec::new(),
            package_detail: Vec::new(),
            package_bbox_x: 0.0,
            package_bbox_y: 0.0,
            model_3d: None,
            manufacturer: "CN Manufacturer".to_string(),
            datasheet: "https://source.test/ds".to_string(),
            jlc_id: String::new(),
            package: String::new(),
        }
    }
}
