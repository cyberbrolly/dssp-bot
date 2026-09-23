"""The live-gate evidence dump (`DSSP_DUMP_DIR`).

Off unless an operator asks for it, and never load-bearing: a run must behave
identically with it unset (the normal case) and with it set. These tests pin the
off switch, the verbatim write, the overwrite, and the one property that matters
for a diagnostic — that gathering evidence can never fail a submission.
"""

from app.portal import constants as c
from app.portal.client import PortalClient


def test_dump_dir_is_none_unless_asked(monkeypatch):
    monkeypatch.delenv("DSSP_DUMP_DIR", raising=False)
    assert c.dump_dir() is None

    monkeypatch.setenv("DSSP_DUMP_DIR", "/tmp/evidence")
    assert c.dump_dir() == "/tmp/evidence"


def test_an_empty_dump_dir_is_off_not_the_cwd(monkeypatch):
    # `DSSP_DUMP_DIR=` must not mean "write portal HTML into the working
    # directory", which is one typo away from committing trainee data.
    monkeypatch.setenv("DSSP_DUMP_DIR", "")
    assert c.dump_dir() is None


def test_nothing_is_written_when_the_dump_is_off(tmp_path, monkeypatch):
    monkeypatch.delenv("DSSP_DUMP_DIR", raising=False)
    monkeypatch.chdir(tmp_path)

    PortalClient()._dump("trainee-list.html", "<html>portal</html>", "unit test")

    assert list(tmp_path.rglob("*")) == []


def test_the_raw_body_is_written_verbatim_when_asked(tmp_path, monkeypatch):
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path / "evidence"))
    body = '<form><input name="__RequestVerificationToken" value="TOK"/></form>'

    PortalClient()._dump("training-form-123.html", body, "unit test")

    assert (tmp_path / "evidence" / "training-form-123.html").read_text() == body


def test_a_dump_that_cannot_be_written_does_not_raise(tmp_path, monkeypatch):
    # A file where the directory should be: mkdir raises NotADirectoryError, an
    # OSError. A diagnostic that can abort a submission is worse than no
    # diagnostic, so this must be swallowed.
    blocker = tmp_path / "evidence"
    blocker.write_text("not a directory")
    monkeypatch.setenv("DSSP_DUMP_DIR", str(blocker))

    PortalClient()._dump("trainee-list.html", "<html/>", "unit test")


def test_a_body_that_is_not_valid_utf8_is_replaced_not_raised(tmp_path, monkeypatch):
    # resp.text() can hand back a lone surrogate when the portal mislabels its
    # charset. Writing that raises UnicodeEncodeError — a ValueError, so it would
    # NOT be caught by an OSError handler, and the submission would die on a
    # diagnostic. The body must go to disk with the offending character replaced.
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))

    PortalClient()._dump("submit-response-1.html", "ok \ud800 done", "unit test")

    assert "ok " in (tmp_path / "submit-response-1.html").read_text()


def test_a_second_dump_overwrites_the_first(tmp_path, monkeypatch):
    # Evidence must describe the run that just happened, not accumulate across
    # runs: stale HTML from an earlier attempt is worse than none.
    monkeypatch.setenv("DSSP_DUMP_DIR", str(tmp_path))
    client = PortalClient()

    client._dump("trainee-list.html", "first", "unit test")
    client._dump("trainee-list.html", "second", "unit test")

    assert (tmp_path / "trainee-list.html").read_text() == "second"
