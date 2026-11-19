"""Persistent Python interpreter for a CLO Rust MCP engine session."""
import contextlib
from enum import IntEnum
import io
import json
import math
import os
import sys
import time
import traceback
import types

protocol_input = sys.stdin
protocol_output = sys.stdout


def read_message():
    line = protocol_input.readline()
    return json.loads(line) if line else None


def send_message(message):
    protocol_output.write(json.dumps(message, allow_nan=False) + "\n")
    protocol_output.flush()


class ApiOptions:
    _qualified_name = ""
    _defaults = {}

    def __init__(self):
        for field, value in self._defaults.items():
            setattr(self, field, value)

    def __setattr__(self, name, value):
        if name not in self._defaults:
            raise AttributeError(f"Unknown option: {name}")
        super().__setattr__(name, value)


class ApiHandle:
    def __init__(self, value):
        self.token = value["$handle"]
        self.name = value["type"]

    def __repr__(self):
        return f"<{self.name}>"


def encode(value):
    if isinstance(value, ApiOptions):
        return {"$type": value._qualified_name, "fields": vars(value)}
    if isinstance(value, ApiHandle):
        return {"$handle": value.token}
    if isinstance(value, dict):
        return {str(key): encode(item) for key, item in value.items()}
    if isinstance(value, (tuple, list, set)):
        return [encode(item) for item in value]
    if isinstance(value, float) and not math.isfinite(value):
        return str(value)
    if value is None or isinstance(value, (str, float, int, bool)):
        return value
    return str(value)


def decode(value):
    if isinstance(value, dict) and "$handle" in value and "type" in value:
        return ApiHandle(value)
    if isinstance(value, dict):
        return {key: decode(item) for key, item in value.items()}
    if isinstance(value, list):
        return [decode(item) for item in value]
    return value


def result_value(value):
    if isinstance(value, ApiOptions):
        return result_value(vars(value))
    if isinstance(value, dict):
        return {str(key): result_value(item) for key, item in value.items()}
    if isinstance(value, (tuple, list, set)):
        return [result_value(item) for item in value]
    return encode(value)


def method(name, doc):
    def invoke(*args, **kwargs):
        send_message({"kind": "api", "name": name, "args": encode(args), "kwargs": encode(kwargs)})
        reply = read_message()
        if reply is None:
            raise EOFError("CLO Rust API connection was closed")
        if not reply["ok"]:
            raise RuntimeError(reply["error"])
        value = decode(reply["value"])
        if name == "utility_api.GetAvatarMeasurements":
            measurement = sys.modules["ApiTypes"].CloApiAvatarMeasurement
            rows = []
            for fields in value:
                row = measurement()
                for field, item in fields.items():
                    setattr(row, field, item)
                rows.append(row)
            return rows
        return value

    invoke.__name__ = name.rsplit(".", 1)[-1]
    invoke.__doc__ = doc
    return invoke


def main():
    modules = {name: types.ModuleType(name) for name in (
        "import_api", "export_api", "fabric_api", "pattern_api", "utility_api", "rest_api", "ApiTypes"
    )}
    setup = read_message()
    if setup is None:
        return
    constants = []
    for entry in setup["entries"]:
        qualified_name = entry["name"]
        module, name = qualified_name.split(".", 1)
        if entry["kind"] == "constant":
            constants.append(entry)
            continue
        if "enum_members" in entry:
            member = IntEnum(name, entry["enum_members"], module=module)
        elif entry["kind"] == "type":
            member = type(name, (ApiOptions,), {
                "__module__": module,
                "_qualified_name": qualified_name,
                "_defaults": entry["defaults"],
            })
        else:
            member = method(qualified_name, entry["summary"])
        setattr(modules[module], name, member)
    for entry in constants:
        module, name = entry["name"].split(".", 1)
        enum_type = getattr(modules[module], entry["enum_type"])
        setattr(modules[module], name, enum_type[entry["enum_member"]])
    sys.modules.update(modules)
    namespace = {
        "__name__": "clo_rust_exec", "json": json, "math": math, "os": os,
        "sys": sys, "time": time, "pump": lambda: None, "log": print, **modules,
    }
    while (message := read_message()) is not None:
        namespace.pop("result", None)
        output = io.StringIO()
        try:
            with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
                try:
                    script = compile(message["code"], "<clo-rust>", "eval")
                except SyntaxError:
                    script = compile(message["code"], "<clo-rust>", "exec")
                    exec(script, namespace)
                    value = namespace.get("result")
                else:
                    value = eval(script, namespace)
            reply = {"kind": "done", "value": result_value(value), "stdout": output.getvalue()}
        except BaseException as error:
            detail = "".join(traceback.format_exception_only(type(error), error)).strip()
            reply = {"kind": "error", "error": detail, "stdout": output.getvalue()}
        send_message(reply)


main()
