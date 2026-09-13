"""Coded portal errors.

Each carries an ``error_code`` from the shared taxonomy (protocol.py). The
worker maps these onto error responses; Rust reads the code to decide whether a
retry is safe. Mirrors src/core/shared/errors.ts.
"""

from __future__ import annotations

from .. import protocol as p


class PortalError(Exception):
    def __init__(self, error_code: str, message: str) -> None:
        super().__init__(message)
        self.error_code = error_code
        self.message = message


def element_not_found(element: str) -> PortalError:
    return PortalError(p.ERR_ELEMENT_NOT_FOUND, f"Portal element not found: {element}")


def missing_data(field: str) -> PortalError:
    return PortalError(p.ERR_MISSING_DATA, f"Required data is missing: {field}")


def session_expired() -> PortalError:
    return PortalError(p.ERR_SESSION_EXPIRED, "The DSSP session has expired.")


def network(message: str) -> PortalError:
    return PortalError(p.ERR_NETWORK, message)


def timeout(operation: str, ms: int) -> PortalError:
    return PortalError(p.ERR_TIMEOUT, f"Timed out after {ms}ms: {operation}")


def validation(message: str) -> PortalError:
    return PortalError(p.ERR_VALIDATION_FAILED, message)


def portal_structure(message: str) -> PortalError:
    return PortalError(p.ERR_PORTAL_STRUCTURE_CHANGED, message)


def trainee_not_found(message: str) -> PortalError:
    return PortalError(p.ERR_TRAINEE_NOT_FOUND, message)


def duplicate(message: str) -> PortalError:
    return PortalError(p.ERR_DUPLICATE_RECORD, message)


def confirmation_unknown(message: str) -> PortalError:
    return PortalError(
        p.ERR_CONFIRMATION_UNKNOWN,
        f"Submitted, but the result could not be confirmed: {message}",
    )
