from validate.patterns import HEX_COLOR, SEMVER, SLUG, collapse_slashes


def test_slug():
    assert SLUG.match("release-notes")
    assert not SLUG.match("Release Notes")


def test_semver():
    assert SEMVER.match("1.2.3-rc.1").group(4) == "rc.1"


def test_hex_color():
    assert HEX_COLOR.match("#1a2B3c")


def test_collapse_slashes():
    assert collapse_slashes("/a//b///c") == "/a/b/c"
