"""Checks the button markup in the page templates and the stylesheet.

The templates are plain HTML, so these tests read them with html.parser
instead of a browser. They pin down what the scripts in static/js rely on:
which buttons carry which classes, and the ids and data attributes that tie
a primary button to its help text and label.
"""

import unittest
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TEMPLATES = ROOT / "templates"
STYLESHEET = ROOT / "static" / "css" / "buttons.css"
PAGES = ("index.html", "reserve.html")


class ElementCollector(HTMLParser):
    """Records the tag and attributes of every start tag, in document order."""

    def __init__(self):
        super().__init__()
        self.elements = []

    def handle_starttag(self, tag, attrs):
        self.elements.append((tag, dict(attrs)))


def parse(name):
    collector = ElementCollector()
    collector.feed((TEMPLATES / name).read_text(encoding="utf-8"))
    collector.close()
    return collector.elements


def classes(attrs):
    return (attrs.get("class") or "").split()


def with_class(elements, name):
    return [(tag, attrs) for tag, attrs in elements if name in classes(attrs)]


class TemplateTests(unittest.TestCase):
    def test_primary_buttons_start_with_the_base_class(self):
        for page in PAGES:
            with self.subTest(page=page):
                found = with_class(parse(page), "btn-accent")
                self.assertTrue(found)
                for _, attrs in found:
                    self.assertEqual(classes(attrs)[0], "btn")

    def test_outline_buttons_are_not_also_primary(self):
        for page in PAGES:
            with self.subTest(page=page):
                for _, attrs in with_class(parse(page), "btn-primary-outline"):
                    self.assertNotIn("btn-accent", classes(attrs))

    def test_search_button_is_large(self):
        (attrs,) = [
            attrs for tag, attrs in with_class(parse("index.html"), "btn-accent") if tag == "button"
        ]
        self.assertEqual(classes(attrs), ["btn", "btn-accent", "btn-large"])

    def test_reserve_button_points_at_its_help_text(self):
        elements = parse("reserve.html")
        (button,) = [attrs for tag, attrs in with_class(elements, "btn-accent") if tag == "button"]
        self.assertEqual(button["aria-describedby"], "btn-primary-help")
        self.assertIn("disabled", button)
        self.assertIn("btn-primary-help", [attrs.get("id") for _, attrs in elements])

    def test_reserve_form_names_the_button_label(self):
        (form,) = [attrs for tag, attrs in parse("reserve.html") if tag == "form"]
        self.assertEqual(form["data-btn-primary-label"], "Reserve this copy")


class StylesheetTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.css = STYLESHEET.read_text(encoding="utf-8")

    def test_hover_and_focus_share_one_rule(self):
        self.assertIn(".btn-accent:hover,\n.btn-accent:focus-visible {", self.css)

    def test_colors_come_from_custom_properties(self):
        for name in ("--btn-primary-bg", "--btn-primary-bg-hover", "--btn-primary-fg"):
            with self.subTest(name=name):
                self.assertIn(f"  {name}: #", self.css)
                self.assertIn(f"var({name})", self.css)

    def test_help_text_has_its_own_rule(self):
        self.assertIn("#btn-primary-help {", self.css)


if __name__ == "__main__":
    unittest.main()
