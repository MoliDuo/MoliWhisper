"""Tests for the on-screen keyboard hotkey switch in HotkeyManager."""

import sys
import types

import pytest


@pytest.fixture
def manager(monkeypatch):
    # HotkeyManager marshals to GTK via GLib.idle_add; run it inline instead
    # so the tests don't need PyGObject.
    glib = types.SimpleNamespace(idle_add=lambda fn, *a: fn(*a), SOURCE_REMOVE=False)
    repository = types.ModuleType("gi.repository")
    repository.GLib = glib
    gi = types.ModuleType("gi")
    gi.repository = repository
    monkeypatch.setitem(sys.modules, "gi", gi)
    monkeypatch.setitem(sys.modules, "gi.repository", repository)
    monkeypatch.delitem(sys.modules, "doubao_murmur.hotkey.manager", raising=False)

    from doubao_murmur.hotkey.manager import HotkeyManager

    m = HotkeyManager()
    m.calls = []
    m.on_keyboard = lambda: m.calls.append("keyboard")
    return m


def test_keyboard_hotkey_fires_by_default(manager):
    manager.trigger_keyboard()
    assert manager.calls == ["keyboard"]


def test_disabled_keyboard_hotkey_is_ignored(manager):
    manager.keyboard_hotkey_enabled = False
    manager.trigger_keyboard()
    assert manager.calls == []


def test_reenabling_takes_effect_immediately(manager):
    manager.keyboard_hotkey_enabled = False
    manager.trigger_keyboard()
    manager.keyboard_hotkey_enabled = True
    manager.trigger_keyboard()
    assert manager.calls == ["keyboard"]
