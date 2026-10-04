"""Offline contracts using synthetic HTML and user-supplied portal facts."""

from urllib.parse import parse_qs, urlsplit

import pytest

from app.portal import constants as c
from app.portal import parsing as parse


@pytest.mark.parametrize(
    ("trainee_id", "encoded"),
    [
        ("123", "123"),
        ("12/3 &other=4?#%+", "12%2F3%20%26other%3D4%3F%23%25%2B"),
    ],
)
def test_training_form_url_encodes_query_value(monkeypatch, trainee_id, encoded):
    monkeypatch.setattr(c, "DSSP_ORIGIN", "https://portal.invalid")
    url = c.training_form_url(trainee_id)
    assert url == f"https://portal.invalid/Trainee/TrainingLog?TraineeId={encoded}"
    parts = urlsplit(url)
    assert parts.path == "/Trainee/TrainingLog"
    assert parse_qs(parts.query) == {"TraineeId": [trainee_id]}
    assert parts.fragment == ""


@pytest.mark.parametrize(
    "training_href",
    [
        "/Trainee/TrainingLog?TraineeId=123",
        "/Trainee/TrainingLog?area=&amp;TraineeId=123",
        "/Trainee/TrainingLog/TraineeId=123",
    ],
)
def test_training_link_preferred_when_details_link_comes_first(training_href):
    soup = parse.parse_html(
        '<table class="table-checkable"><tbody><tr><td>'
        '<a href="/Trainee/Details?TraineeId=999">Details</a>'
        f'<a href="{training_href}">Training log</a>'
        '</td></tr></tbody></table>'
    )
    trainees = parse.get_trainees(soup)
    assert len(trainees) == 1
    assert trainees[0]["id"] == "123"
    assert trainees[0]["trainee_id"] == "123"
    assert trainees[0]["profile_url"] == training_href.replace("&amp;", "&")


def test_training_form_and_submit_contract():
    soup = parse.parse_html(
        '<form id="logoutForm" action="/Account/LogOff?area="></form>'
        '<div id="frmtraininglog"></div>'
        '<form id="frmtraininglog"></form>'
    )
    form = soup.select_one(c.TRAINING_FORM_SELECTOR)
    assert form is not None
    assert form.name == "form"
    assert form["id"] == "frmtraininglog"
    assert not form.has_attr("method")
    assert not form.has_attr("action")
    assert c.TRAINING_SUBMIT_PATH == "/Trainee/LogTraining"


@pytest.mark.parametrize("attribute", ["id", "name"])
@pytest.mark.parametrize(
    "field", ["TrainingOptionId", "TrainingType", "TrainingTypeId", "TrainingTypeID"]
)
def test_training_type_selectors_preserve_alternatives(attribute, field):
    soup = parse.parse_html(
        f'<select {attribute}="{field}"><option value="2">Theory</option></select>'
    )
    select = parse.find_training_type_select(soup)
    assert select is not None
    assert select[attribute] == field
    assert any(soup.select_one(selector) is select for selector in c.TRAINING_TYPE_SELECTORS)


@pytest.mark.parametrize(
    "html",
    [
        '<form id="logoutForm" action="/Account/LogOff?area="></form>',
        '<form id="logoutForm"></form>',
        '<form action="/Account/LogOff?area="></form>',
        '<a href="/Account/LogOff?area=">Sign out</a>',
        '<form action="/Account/Logout"></form>',
        '<a href="/Account/Logout">Sign out</a>',
    ],
)
def test_logout_markers_indicate_authenticated_page(html):
    soup = parse.parse_html(html)
    assert parse.has_authenticated_marker(soup)
    assert not parse.is_login_page(soup, c.training_form_url("123"))


@pytest.mark.parametrize(
    ("html", "path"),
    [
        ('<form id="loginForm"></form>', "/Trainee"),
        ('<form action="/Account/Login?ReturnUrl=/Trainee"></form>', "/Trainee"),
        ("<p>Please sign in</p>", "/Account/Login?ReturnUrl=/Trainee"),
    ],
)
def test_login_detection_remains_correct(html, path):
    soup = parse.parse_html(html)
    assert parse.is_login_page(soup, f"https://portal.invalid{path}")
    assert not parse.has_authenticated_marker(soup)


def test_login_detection_is_not_hidden_by_logout_marker():
    soup = parse.parse_html(
        '<form id="loginForm" action="/Account/Login"></form>'
        '<form id="logoutForm" action="/Account/LogOff?area="></form>'
    )
    assert parse.is_login_page(soup, c.training_form_url("123"))
