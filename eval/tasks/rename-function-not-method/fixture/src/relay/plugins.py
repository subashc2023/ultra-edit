"""Plugin discovery for the relay service.

A plugin is a module in the relay_plugins package that defines setup(settings).
Plugins are imported by name, so each deployment loads only the plugins listed
in its settings.
"""

import importlib

PACKAGE = "relay_plugins"


def load(name):
    """Import the plugin module called name and return it."""
    module = importlib.import_module(f"{PACKAGE}.{name}")
    if not callable(getattr(module, "setup", None)):
        raise ImportError(f"plugin {name!r} does not define setup()")
    return module


def load_all(names):
    """Import every plugin in names, in the order given."""
    return [load(name) for name in names]
