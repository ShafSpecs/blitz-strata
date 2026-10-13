use crate::{BaseDocument, ElementData, Node as BlitzDomNode, local_name};
use accesskit::{Affine, Node as AccessKitNode, NodeId, Rect, Role, TreeId, TreeInfo, TreeUpdate};
use style::properties::longhands::visibility;

impl BaseDocument {
    pub fn build_accessibility_tree(&self) -> TreeUpdate {
        let mut nodes = std::collections::HashMap::new();
        let mut window = AccessKitNode::new(Role::Window);
        // Node bounds are in CSS pixels; the window maps them to device pixels.
        window.set_transform(Affine::scale(self.viewport.scale_f64()));
        let mut hidden_nodes = std::collections::HashSet::new();

        self.visit(|node_id, node| {
            if node.is_hidden_from_accessibility_tree()
                || node
                    .parent
                    .map(|p| hidden_nodes.contains(&p))
                    .unwrap_or(false)
            {
                hidden_nodes.insert(node_id);
                return;
            }
            let parent = node
                .parent
                .and_then(|parent_id| nodes.get_mut(&parent_id))
                .map(|(_, parent)| parent)
                .unwrap_or(&mut window);
            let (id, builder) = self.build_accessibility_node(node, parent);

            nodes.insert(node_id, (id, builder));
        });

        let mut nodes: Vec<_> = nodes
            .into_iter()
            .map(|(_, (id, node))| (id, node))
            .collect();
        nodes.push((NodeId(u64::MAX), window));

        let tree = TreeInfo::new(NodeId(u64::MAX));
        TreeUpdate {
            tree_id: TreeId::ROOT,
            nodes,
            tree: Some(tree),
            focus: NodeId(self.focus_node_id.map(|id| id.as_u64()).unwrap_or(u64::MAX)),
        }
    }

    fn build_accessibility_node(
        &self,
        node: &BlitzDomNode,
        parent: &mut AccessKitNode,
    ) -> (NodeId, AccessKitNode) {
        let id = NodeId(node.id.as_u64());

        let mut builder = AccessKitNode::default();
        if node.parent.is_none() {
            builder.set_role(Role::Window)
        } else if let Some(element_data) = node.element_data() {
            let name = element_data.name.local.to_string();
            let role_attr = element_data.attr(local_name!("role"));

            // TODO: The roles of elements with strong native semantics cannot be overridden; see
            // https://www.w3.org/TR/wai-aria-1.2/#host_general_conflict.
            let role = role_attr
                .and_then(role_from_name)
                .or_else(|| role_from_element_data(element_data))
                .unwrap_or(Role::Unknown);

            builder.set_role(role);
            builder.set_html_tag(name);

            if let Some(label) = self.accessible_name(node, element_data, role) {
                builder.set_label(label);
            }
            if let Some(rect) = self.get_client_bounding_rect(node.id) {
                if rect.width > 0.0 || rect.height > 0.0 {
                    builder.set_bounds(Rect {
                        x0: rect.x,
                        y0: rect.y,
                        x1: rect.x + rect.width,
                        y1: rect.y + rect.height,
                    });
                }
            }

            // https://www.w3.org/TR/wai-aria-1.2/#tree_exclusion
            if element_data.attr(local_name!("aria-hidden")) == Some("true") {
                builder.set_hidden();
            }
        } else if node.is_text_node() {
            builder.set_role(Role::TextRun);
            builder.set_value(node.text_content());
            parent.push_labelled_by(id)
        }

        parent.push_child(id);

        (id, builder)
    }
}

impl BaseDocument {
    /// The element's accessible name: a subset of
    /// https://www.w3.org/TR/accname-1.2/ in priority order (`aria-labelledby`,
    /// `aria-label`, native labels such as `<label>` and `alt`, name from
    /// content for the roles that allow it, then `title` and `placeholder`).
    fn accessible_name(
        &self,
        node: &BlitzDomNode,
        element: &ElementData,
        role: Role,
    ) -> Option<String> {
        let non_empty = |s: String| {
            let s = collapse_whitespace(&s);
            (!s.is_empty()).then_some(s)
        };

        if let Some(ids) = element.attr(local_name!("aria-labelledby")) {
            let text: Vec<String> = ids
                .split_ascii_whitespace()
                .filter_map(|id| self.get_element_by_id(id))
                .map(|id| self.nodes[id].text_content())
                .collect();
            if let Some(name) = non_empty(text.join(" ")) {
                return Some(name);
            }
        }
        if let Some(name) = element.attr(local_name!("aria-label")).map(str::to_owned).and_then(non_empty) {
            return Some(name);
        }

        let tag = &element.name.local;
        let is_labelable = *tag == local_name!("input")
            || *tag == local_name!("textarea")
            || *tag == local_name!("select")
            || *tag == local_name!("button")
            || *tag == local_name!("meter")
            || *tag == local_name!("progress");
        if is_labelable && element.attr(local_name!("type")) != Some("hidden") {
            if let Some(name) = self.label_text(node, element).and_then(non_empty) {
                return Some(name);
            }
        }
        if *tag == local_name!("img") || element.attr(local_name!("type")) == Some("image") {
            if let Some(alt) = element.attr(local_name!("alt")) {
                return non_empty(alt.to_owned());
            }
        }
        if name_from_content(role) {
            if let Some(name) = non_empty(self.text_for_name(node)) {
                return Some(name);
            }
        }
        element
            .attr(local_name!("title"))
            .or_else(|| element.attr(local_name!("placeholder")))
            .map(str::to_owned)
            .and_then(non_empty)
    }

    /// A node's text for name computation: descendant text, with a space
    /// around the content of children that aren't inline (accname 2F: block
    /// and flex children read as separate words, e.g. "CI-065 14.8").
    fn text_for_name(&self, node: &BlitzDomNode) -> String {
        let mut out = String::new();
        for &child_id in node.children.iter() {
            let child = &self.nodes[child_id];
            if child.is_text_node() {
                out.push_str(&child.text_content());
            } else if child.element_data().is_some() && !child.is_hidden_from_accessibility_tree() {
                let inline = child
                    .display_style()
                    .is_some_and(|d| d.outside() == style::values::specified::box_::DisplayOutside::Inline
                        && d.inside() == style::values::specified::box_::DisplayInside::Flow);
                let text = self.text_for_name(child);
                if inline {
                    out.push_str(&text);
                } else {
                    out.push(' ');
                    out.push_str(&text);
                    out.push(' ');
                }
            }
        }
        out
    }

    /// Text of the `<label>` elements associated with a labelable element:
    /// those whose `for` names its id, and an ancestor `<label>`.
    fn label_text(&self, node: &BlitzDomNode, element: &ElementData) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(id) = element.attr(local_name!("id")).filter(|id| !id.is_empty()) {
            for (_, candidate) in self.nodes.iter() {
                if let Some(el) = candidate.element_data() {
                    if el.name.local == local_name!("label") && el.attr(local_name!("for")) == Some(id) {
                        parts.push(self.text_for_name(candidate));
                    }
                }
            }
        }
        let mut ancestor = node.parent;
        while let Some(id) = ancestor {
            let candidate = &self.nodes[id];
            if candidate.element_data().is_some_and(|el| el.name.local == local_name!("label")) {
                parts.push(self.text_for_name(candidate));
                break;
            }
            ancestor = candidate.parent;
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }
}

/// Roles whose name may come from their content (accname, "name from content").
fn name_from_content(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::Cell
            | Role::CheckBox
            | Role::ColumnHeader
            | Role::GridCell
            | Role::Heading
            | Role::Link
            | Role::ListBoxOption
            | Role::MenuItem
            | Role::MenuItemCheckBox
            | Role::MenuItemRadio
            | Role::RadioButton
            | Role::Row
            | Role::RowHeader
            | Role::Switch
            | Role::Tab
            | Role::Tooltip
            | Role::TreeItem
    )
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl BlitzDomNode {
    // https://www.w3.org/TR/wai-aria-1.2/#tree_exclusion
    fn is_hidden_from_accessibility_tree(&self) -> bool {
        self.try_stylo_element_data()
            .as_ref()
            .and_then(|s| s.get())
            // A node created since the last style pass has no style yet; the tree
            // is rebuilt once it has one.
            .and_then(|s| s.styles.get_primary().cloned())
            .map(|primary| {
                primary.get_box().clone_display().is_none()
                    || primary.clone_visibility() == visibility::computed_value::T::Hidden
            })
            .unwrap_or(false)
    }
}

fn role_from_name(name: &str) -> Option<Role> {
    match name {
        "alert" => Some(Role::Alert),
        "alertdialog" => Some(Role::AlertDialog),
        "button" => Some(Role::Button),
        "checkbox" => Some(Role::CheckBox),
        "dialog" => Some(Role::Dialog),
        "gridcell" => Some(Role::GridCell),
        "link" => Some(Role::Link),
        "log" => Some(Role::Log),
        "marquee" => Some(Role::Marquee),
        "menuitem" => Some(Role::MenuItem),
        "menuitemcheckbox" => Some(Role::MenuItemCheckBox),
        "menuitemradio" => Some(Role::MenuItemRadio),
        "option" => Some(Role::ListBoxOption),
        "progressbar" => Some(Role::ProgressIndicator),
        "radio" => Some(Role::RadioButton),
        "scrollbar" => Some(Role::ScrollBar),
        "slider" => Some(Role::Slider),
        "spinbutton" => Some(Role::SpinButton),
        "status" => Some(Role::Status),
        "tab" => Some(Role::Tab),
        "tabpanel" => Some(Role::TabPanel),
        "textbox" => Some(Role::TextInput),
        "timer" => Some(Role::Timer),
        "tooltip" => Some(Role::Tooltip),
        "treeitem" => Some(Role::TreeItem),
        "combobox" => Some(Role::ComboBox),
        "grid" => Some(Role::Grid),
        "listbox" => Some(Role::ListBox),
        "menu" => Some(Role::Menu),
        "menubar" => Some(Role::MenuBar),
        "radiogroup" => Some(Role::RadioGroup),
        "tablist" => Some(Role::TabList),
        "tree" => Some(Role::Tree),
        "treegrid" => Some(Role::TreeGrid),
        "article" => Some(Role::Article),
        "columnheader" => Some(Role::ColumnHeader),
        "definition" => Some(Role::Definition),
        "document" => Some(Role::Document),
        "group" => Some(Role::Group),
        "heading" => Some(Role::Heading),
        "img" => Some(Role::Image),
        "list" => Some(Role::List),
        "listitem" => Some(Role::ListItem),
        "math" => Some(Role::Math),
        "note" => Some(Role::Note),
        "region" => Some(Role::Region),
        "row" => Some(Role::Row),
        "rowgroup" => Some(Role::RowGroup),
        "rowheader" => Some(Role::RowHeader),
        "toolbar" => Some(Role::Toolbar),
        "application" => Some(Role::Application),
        "banner" => Some(Role::Banner),
        "complementary" => Some(Role::Complementary),
        "contentinfo" => Some(Role::ContentInfo),
        "form" => Some(Role::Form),
        "main" => Some(Role::Main),
        "navigation" => Some(Role::Navigation),
        "search" => Some(Role::Search),
        _ => None,
    }
}

fn role_from_element_data(element_data: &ElementData) -> Option<Role> {
    // <https://www.w3.org/TR/html-aam-1.0/>
    match &*element_data.name.local {
        // Document structure
        "article" => Some(Role::Article),
        "aside" => Some(Role::Complementary),
        "footer" => Some(Role::Footer),
        "header" => Some(Role::Header),
        "main" => Some(Role::Main),
        "nav" => Some(Role::Navigation),
        "search" => Some(Role::Search),
        "section" => Some(Role::Section),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => Some(Role::Heading),
        "p" => Some(Role::Paragraph),
        "blockquote" => Some(Role::Blockquote),
        "figure" => Some(Role::Figure),
        "figcaption" | "caption" => Some(Role::Caption),
        "hr" => Some(Role::Splitter),

        // Grouping
        "ul" | "ol" | "menu" => Some(Role::List),
        "li" => Some(Role::ListItem),
        "dl" => Some(Role::DescriptionList),
        "dt" => Some(Role::Term),
        "dd" => Some(Role::Definition),
        "dialog" => Some(Role::Dialog),
        "fieldset" => Some(Role::Group),
        "form" => Some(Role::Form),
        "div" => Some(Role::GenericContainer),

        // Tables
        "table" => Some(Role::Table),
        "thead" | "tbody" | "tfoot" => Some(Role::RowGroup),
        "tr" => Some(Role::Row),
        "td" => Some(Role::Cell),
        "th" => match element_data.attr(local_name!("scope")) {
            Some("row") | Some("rowgroup") => Some(Role::RowHeader),
            _ => Some(Role::ColumnHeader),
        },

        // Interactive
        // An <a> is only a link when it has an href.
        "a" => match element_data.attr(local_name!("href")) {
            Some(_) => Some(Role::Link),
            None => Some(Role::GenericContainer),
        },
        "button" => Some(Role::Button),
        "label" => Some(Role::Label),
        "legend" => Some(Role::Label),
        "select" => match element_data.attr(local_name!("multiple")) {
            Some(_) => Some(Role::ListBox),
            None => Some(Role::ComboBox),
        },
        "option" => Some(Role::ListBoxOption),
        "textarea" => Some(Role::MultilineTextInput),
        "progress" => Some(Role::ProgressIndicator),
        "meter" => Some(Role::Meter),
        "output" => Some(Role::Status),
        "summary" => Some(Role::DisclosureTriangle),

        // Inline semantics
        "code" => Some(Role::Code),
        "em" => Some(Role::Emphasis),
        "strong" => Some(Role::Strong),
        "mark" => Some(Role::Mark),
        "time" => Some(Role::Time),
        "img" => Some(Role::Image),
        "iframe" => Some(Role::Iframe),

        "input" => {
            let ty = element_data.attr(local_name!("type")).unwrap_or("text");
            match ty {
                "button" | "submit" | "reset" => Some(Role::Button),
                "checkbox" => Some(Role::CheckBox),
                "color" => Some(Role::ColorWell),
                "date" => Some(Role::DateInput),
                "datetime-local" => Some(Role::DateTimeInput),
                "email" => Some(Role::EmailInput),
                "number" => Some(Role::NumberInput),
                "password" => Some(Role::PasswordInput),
                "radio" => Some(Role::RadioButton),
                "range" => Some(Role::Slider),
                "search" => Some(Role::SearchInput),
                "tel" => Some(Role::PhoneNumberInput),
                "time" => Some(Role::TimeInput),
                _ => Some(Role::TextInput),
            }
        }
        _ => None,
    }
}
