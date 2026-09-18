"""Safe trainee matching tests — no browser, no network."""

import pytest

from app.portal import errors as e
from app.portal.client import PortalClient

ROWS = [
    {"id": "123", "name": "John Doe"},
    {"id": "456", "name": "Jane Roe"},
    {"id": "789", "name": "JOHN   DOE"},
]


def make_client(rows=ROWS):
    client = PortalClient()
    client.list_trainees = lambda: rows  # bypasses the browser on purpose
    return client


def test_resolve_by_id():
    resolved = make_client()._resolve_trainee({"id": "456"})
    assert resolved["name"] == "Jane Roe"


def test_resolve_by_normalized_name():
    resolved = make_client()._resolve_trainee({"name": "jane  roe"})
    assert resolved["id"] == "456"


def test_unknown_id_stops():
    with pytest.raises(e.PortalError) as info:
        make_client()._resolve_trainee({"id": "999"})
    assert info.value.error_code == "TRAINEE_NOT_FOUND"


def test_unknown_name_stops():
    with pytest.raises(e.PortalError) as info:
        make_client()._resolve_trainee({"name": "Nobody Here"})
    assert info.value.error_code == "TRAINEE_NOT_FOUND"


def test_ambiguous_name_never_guesses():
    with pytest.raises(e.PortalError) as info:
        make_client()._resolve_trainee({"name": "John   Doe"})
    assert info.value.error_code == "TRAINEE_NOT_FOUND"
    assert "Ambiguous" in info.value.message


def test_missing_selector_is_missing_data():
    with pytest.raises(e.PortalError) as info:
        make_client()._resolve_trainee({})
    assert info.value.error_code == "MISSING_DATA"
