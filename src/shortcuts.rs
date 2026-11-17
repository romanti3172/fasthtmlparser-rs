//! The one-call shortcuts of z3l.FastHtmlParser (find_by_name_class, is_name_itemprop, parent_by_name,
//! attribute_title, ...). The name is a [`Tag`] or, for any other name, a string:
//! `doc.find_by_name_class(Tag::DIV, "x")`, `doc.find_by_name_class("my-widget", "x")`.

use std::borrow::Cow;

use crate::{any, attr, Document, Node, Query};

// The older "optional" shortcuts skip an empty criterion.

fn opt_class<'q>(q: Query<'q>, class: &'q str) -> Query<'q> {
    if class.is_empty() {
        q
    } else {
        q.class(class)
    }
}

fn opt_id<'q>(q: Query<'q>, id: &'q str) -> Query<'q> {
    if id.is_empty() {
        q
    } else {
        q.id(id)
    }
}

fn opt_text<'q>(q: Query<'q>, text: &'q str) -> Query<'q> {
    if text.is_empty() {
        q
    } else {
        q.text(text)
    }
}

fn opt_body<'q>(q: Query<'q>, body: &'q str) -> Query<'q> {
    if body.is_empty() {
        q
    } else {
        q.body(body)
    }
}

/// The search shortcuts, on a [`Document`] (the whole page) and on a [`Node`] (the tags inside it).
macro_rules! search_shortcuts {
    ([$($gen:tt)*] $self_ty:ty, $lt:lifetime, $where:literal) => {
        impl<$($gen)*> $self_ty {
            #[doc = concat!("The first tag with the id ", $where, "; None for an empty id.")]
            pub fn find_by_id(&self, id: &str) -> Option<Node<$lt>> {
                if id.is_empty() {
                    return None;
                }
                self.find(any().id(id))
            }

            #[doc = concat!("The first tag with the name ", $where, ".")]
            pub fn find_by_name<'q>(&self, name: impl Into<Query<'q>>) -> Option<Node<$lt>> {
                self.find(name.into())
            }

            #[doc = concat!("The first tag with the name and the inner HTML ", $where, ".")]
            pub fn find_by_name_body<'q>(&self, name: impl Into<Query<'q>>, body: &'q str) -> Option<Node<$lt>> {
                self.find(name.into().body(body))
            }

            #[doc = concat!("The first tag with the name and the decoded attribute value ", $where, ".")]
            pub fn find_by_name_attribute<'q>(
                &self,
                name: impl Into<Query<'q>>,
                attribute: &'q str,
                value: &'q str,
            ) -> Option<Node<$lt>> {
                self.find(name.into().attr(attribute, value))
            }

            #[doc = concat!("The first tag with the name and the class ", $where, ".")]
            pub fn find_by_name_class<'q>(&self, name: impl Into<Query<'q>>, class: &'q str) -> Option<Node<$lt>> {
                self.find(name.into().class(class))
            }

            #[doc = concat!("The first tag with the name, the class and the inner HTML ", $where, ".")]
            pub fn find_by_name_class_body<'q>(
                &self,
                name: impl Into<Query<'q>>,
                class: &'q str,
                body: &'q str,
            ) -> Option<Node<$lt>> {
                self.find(name.into().class(class).body(body))
            }

            #[doc = concat!("The first tag with the name, and the class and visible text when they are not empty ", $where, ".")]
            pub fn find_by_name_class_text<'q>(
                &self,
                name: impl Into<Query<'q>>,
                class: &'q str,
                text: &'q str,
            ) -> Option<Node<$lt>> {
                self.find(opt_text(opt_class(name.into(), class), text))
            }

            #[doc = concat!("The first tag with the name, and the id and visible text when they are not empty ", $where, ".")]
            pub fn find_by_name_id_text<'q>(
                &self,
                name: impl Into<Query<'q>>,
                id: &'q str,
                text: &'q str,
            ) -> Option<Node<$lt>> {
                self.find(opt_text(opt_id(name.into(), id), text))
            }

            #[doc = concat!("The first tag with the name and the visible text ", $where, ".")]
            pub fn find_by_name_text<'q>(&self, name: impl Into<Query<'q>>, text: &'q str) -> Option<Node<$lt>> {
                self.find(name.into().text(text))
            }

            #[doc = concat!("The first tag with the name and the id ", $where, ".")]
            pub fn find_by_name_id<'q>(&self, name: impl Into<Query<'q>>, id: &'q str) -> Option<Node<$lt>> {
                self.find(name.into().id(id))
            }

            #[doc = concat!("The first tag with the name whose id starts with the prefix ", $where, ".")]
            pub fn find_by_name_id_starts_with<'q>(
                &self,
                name: impl Into<Query<'q>>,
                id_prefix: &'q str,
            ) -> Option<Node<$lt>> {
                self.find(name.into().id_starts_with(id_prefix))
            }

            #[doc = concat!("The first tag with the name whose inner HTML starts with the prefix ", $where, ".")]
            pub fn find_by_name_body_starts_with<'q>(
                &self,
                name: impl Into<Query<'q>>,
                body_prefix: &'q str,
            ) -> Option<Node<$lt>> {
                self.find(name.into().body_starts_with(body_prefix))
            }

            #[doc = concat!("The first tag with the name whose visible text starts with the prefix ", $where, "; None for an empty prefix.")]
            pub fn find_by_name_text_starts_with<'q>(
                &self,
                name: impl Into<Query<'q>>,
                text_prefix: &'q str,
            ) -> Option<Node<$lt>> {
                if text_prefix.is_empty() {
                    return None;
                }
                self.find(name.into().text_starts_with(text_prefix))
            }

            #[doc = concat!("The first tag with the name and the class whose visible text starts with the prefix ", $where, "; None for an empty prefix.")]
            pub fn find_by_name_class_text_starts_with<'q>(
                &self,
                name: impl Into<Query<'q>>,
                class: &'q str,
                text_prefix: &'q str,
            ) -> Option<Node<$lt>> {
                if text_prefix.is_empty() {
                    return None;
                }
                self.find(name.into().class(class).text_starts_with(text_prefix))
            }

            #[doc = concat!("The first tag with the name and the itemprop ", $where, ".")]
            pub fn find_by_name_itemprop<'q>(&self, name: impl Into<Query<'q>>, itemprop: &'q str) -> Option<Node<$lt>> {
                self.find(name.into().attr(attr::ITEMPROP, itemprop))
            }

            #[doc = concat!("All tags with the name ", $where, ".")]
            pub fn find_all_by_name<'q>(&self, name: impl Into<Query<'q>>) -> Vec<Node<$lt>> {
                self.find_all(name.into())
            }

            #[doc = concat!("All tags with the name and the decoded attribute value ", $where, ".")]
            pub fn find_all_by_name_attribute<'q>(
                &self,
                name: impl Into<Query<'q>>,
                attribute: &'q str,
                value: &'q str,
            ) -> Vec<Node<$lt>> {
                self.find_all(name.into().attr(attribute, value))
            }

            #[doc = concat!("All tags with the name and the class ", $where, ".")]
            pub fn find_all_by_name_class<'q>(&self, name: impl Into<Query<'q>>, class: &'q str) -> Vec<Node<$lt>> {
                self.find_all(name.into().class(class))
            }

            #[doc = concat!("All tags with the name and the id ", $where, ".")]
            pub fn find_all_by_name_id<'q>(&self, name: impl Into<Query<'q>>, id: &'q str) -> Vec<Node<$lt>> {
                self.find_all(name.into().id(id))
            }

            #[doc = concat!("All tags with the name whose id starts with the prefix ", $where, ".")]
            pub fn find_all_by_name_id_starts_with<'q>(
                &self,
                name: impl Into<Query<'q>>,
                id_prefix: &'q str,
            ) -> Vec<Node<$lt>> {
                self.find_all(name.into().id_starts_with(id_prefix))
            }

            #[doc = concat!("All tags with the name and the itemprop ", $where, ".")]
            pub fn find_all_by_name_itemprop<'q>(&self, name: impl Into<Query<'q>>, itemprop: &'q str) -> Vec<Node<$lt>> {
                self.find_all(name.into().attr(attr::ITEMPROP, itemprop))
            }

            #[doc = concat!("All tags with the name whose inner HTML starts with the prefix ", $where, ".")]
            pub fn find_all_by_name_body_starts_with<'q>(
                &self,
                name: impl Into<Query<'q>>,
                body_prefix: &'q str,
            ) -> Vec<Node<$lt>> {
                self.find_all(name.into().body_starts_with(body_prefix))
            }
        }
    };
}

search_shortcuts!([] Document, '_, "in the document");
search_shortcuts!(['a] Node<'a>, 'a, "inside the tag");

impl<'a> Node<'a> {
    /// Whether the tag has the name and the decoded attribute value.
    pub fn is_name_attribute<'q>(
        &self,
        name: impl Into<Query<'q>>,
        attribute: &'q str,
        value: &'q str,
    ) -> bool {
        self.is(name.into().attr(attribute, value))
    }

    /// Whether the tag has the name and the inner HTML.
    pub fn is_name_body<'q>(&self, name: impl Into<Query<'q>>, body: &'q str) -> bool {
        self.is(name.into().body(body))
    }

    /// Whether the tag has the name and the visible text.
    pub fn is_name_text<'q>(&self, name: impl Into<Query<'q>>, text: &'q str) -> bool {
        self.is(name.into().text(text))
    }

    /// Whether the tag has the name and the class.
    pub fn is_name_class<'q>(&self, name: impl Into<Query<'q>>, class: &'q str) -> bool {
        self.is(name.into().class(class))
    }

    /// Whether the tag has the name, the class and the inner HTML.
    pub fn is_name_class_body<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
        body: &'q str,
    ) -> bool {
        self.is(name.into().class(class).body(body))
    }

    /// Whether the tag has the name, and the class, id and inner HTML when they are not empty.
    pub fn is_name_class_id_body<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
        id: &'q str,
        body: &'q str,
    ) -> bool {
        self.is(opt_body(opt_id(opt_class(name.into(), class), id), body))
    }

    /// Whether the tag has the name, and the class and visible text when they are not empty.
    pub fn is_name_class_text<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
        text: &'q str,
    ) -> bool {
        self.is(opt_text(opt_class(name.into(), class), text))
    }

    /// Whether the tag has the name, and the id and visible text when they are not empty.
    pub fn is_name_id_text<'q>(
        &self,
        name: impl Into<Query<'q>>,
        id: &'q str,
        text: &'q str,
    ) -> bool {
        self.is(opt_text(opt_id(name.into(), id), text))
    }

    /// Whether the tag has the name and the id.
    pub fn is_name_id<'q>(&self, name: impl Into<Query<'q>>, id: &'q str) -> bool {
        self.is(name.into().id(id))
    }

    /// Whether the tag has the name and its visible text starts with the prefix.
    pub fn is_name_text_starts_with<'q>(
        &self,
        name: impl Into<Query<'q>>,
        text_prefix: &'q str,
    ) -> bool {
        self.is(name.into().text_starts_with(text_prefix))
    }

    /// Whether the tag has the name and its inner HTML starts with the prefix.
    pub fn is_name_body_starts_with<'q>(
        &self,
        name: impl Into<Query<'q>>,
        body_prefix: &'q str,
    ) -> bool {
        self.is(name.into().body_starts_with(body_prefix))
    }

    /// Whether the tag has the name and the class and its visible text starts with the prefix.
    pub fn is_name_class_text_starts_with<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
        text_prefix: &'q str,
    ) -> bool {
        self.is(name.into().class(class).text_starts_with(text_prefix))
    }

    /// Whether the tag has the name and its id starts with the prefix.
    pub fn is_name_id_starts_with<'q>(
        &self,
        name: impl Into<Query<'q>>,
        id_prefix: &'q str,
    ) -> bool {
        self.is(name.into().id_starts_with(id_prefix))
    }

    /// Whether the tag has the name and the itemprop.
    pub fn is_name_itemprop<'q>(&self, name: impl Into<Query<'q>>, itemprop: &'q str) -> bool {
        self.is(name.into().attr(attr::ITEMPROP, itemprop))
    }

    /// Whether the tag has the name and its opening tag (without '<' and '>') equals source.
    pub fn is_name_source<'q>(&self, name: impl Into<Query<'q>>, source: &'q str) -> bool {
        self.is(name.into().source(source))
    }

    /// The nearest enclosing tag with the name.
    pub fn parent_by_name<'q>(&self, name: impl Into<Query<'q>>) -> Option<Node<'a>> {
        self.parent_matching(name.into())
    }

    /// The nearest enclosing tag with the name and the class.
    pub fn parent_by_name_class<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
    ) -> Option<Node<'a>> {
        self.parent_matching(name.into().class(class))
    }

    /// The first tag with the name after the end of this one.
    pub fn next_by_name<'q>(&self, name: impl Into<Query<'q>>) -> Option<Node<'a>> {
        self.next_matching(name.into())
    }

    /// The first tag with the name and the class after the end of this one.
    pub fn next_by_name_class<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
    ) -> Option<Node<'a>> {
        self.next_matching(name.into().class(class))
    }

    /// The next sibling with the name.
    pub fn next_sibling_by_name<'q>(&self, name: impl Into<Query<'q>>) -> Option<Node<'a>> {
        self.next_sibling_matching(name.into())
    }

    /// The next sibling with the name and the class.
    pub fn next_sibling_by_name_class<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
    ) -> Option<Node<'a>> {
        self.next_sibling_matching(name.into().class(class))
    }

    /// The nearest previous sibling with the name.
    pub fn previous_sibling_by_name<'q>(&self, name: impl Into<Query<'q>>) -> Option<Node<'a>> {
        self.previous_sibling_matching(name.into())
    }

    /// The source between the end of this tag and the next tag with the name.
    pub fn text_after_until_tag_by_name<'q>(&self, name: impl Into<Query<'q>>) -> &'a str {
        self.text_after_until(name.into())
    }

    /// The source between the end of this tag and the next tag with the name and the class.
    pub fn text_after_until_tag_by_name_class<'q>(
        &self,
        name: impl Into<Query<'q>>,
        class: &'q str,
    ) -> &'a str {
        self.text_after_until(name.into().class(class))
    }

    /// The decoded value of the id attribute.
    pub fn attribute_id(&self) -> Cow<'a, str> {
        self.attr(attr::ID)
    }

    /// The decoded value of the name attribute.
    pub fn attribute_name(&self) -> Cow<'a, str> {
        self.attr(attr::NAME)
    }

    /// The decoded value of the title attribute.
    pub fn attribute_title(&self) -> Cow<'a, str> {
        self.attr(attr::TITLE)
    }

    /// The decoded value of the value attribute.
    pub fn attribute_value(&self) -> Cow<'a, str> {
        self.attr(attr::VALUE)
    }

    /// The decoded value of the style attribute.
    pub fn attribute_style(&self) -> Cow<'a, str> {
        self.attr(attr::STYLE)
    }

    /// The decoded value of the itemprop attribute.
    pub fn attribute_item_prop(&self) -> Cow<'a, str> {
        self.attr(attr::ITEMPROP)
    }
}

/// The tags of the list at the level of the first one (the outermost tags of a find_all result).
pub fn first_level<'a>(nodes: &[Node<'a>]) -> Vec<Node<'a>> {
    first_level_matching(nodes, any(), false)
}

/// The tags of the list with the name, at the level of the first tag of the list.
pub fn first_level_by_name<'a, 'q>(
    nodes: &[Node<'a>],
    name: impl Into<Query<'q>>,
) -> Vec<Node<'a>> {
    first_level_matching(nodes, name.into(), true)
}

/// The tags of the list with the name and the class, at the level of the first tag of the list.
pub fn first_level_by_name_class<'a, 'q>(
    nodes: &[Node<'a>],
    name: impl Into<Query<'q>>,
    class: &'q str,
) -> Vec<Node<'a>> {
    first_level_matching(nodes, name.into().class(class), true)
}

fn first_level_matching<'a>(nodes: &[Node<'a>], q: Query<'_>, check: bool) -> Vec<Node<'a>> {
    let Some(first) = nodes.first() else {
        return Vec::new();
    };
    let level = first.depth();
    nodes
        .iter()
        .copied()
        .filter(|n| n.depth() == level && (!check || q.matches(*n)))
        .collect()
}
