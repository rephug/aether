# Minimal structural shapes from the pinned public Jinja indexing failure.
from typing import overload


def left():
    return 1


def right():
    return 2


def async_variant(normal_func):
    def decorator(async_func):
        if normal_func:
            def is_async(args):
                return left()
        else:
            def is_async(args):
                return right()
        return is_async
    return decorator


class CodeGenerator:
    def _make_finalize(self):
        if self.enabled:
            if self.pass_arg is None:
                def finalize(value):
                    return left()
            else:
                if self.pass_arg == "environment":
                    def finalize(value):
                        return right()
        return finalize


if VERSION >= (3, 13):
    def _get_zipimporter_files(z):
        return left()
else:
    def _get_zipimporter_files(z):
        return right()


class Parser:
    @overload
    def parse_assign_target(self, value=True): ...

    def parse_assign_target(self, value=True):
        return right()

    def parse_tuple(self, simplified):
        if simplified:
            def parse():
                return left()
        else:
            def parse():
                return right()
        return parse()


def urlize(trim_url_limit):
    if trim_url_limit is not None:
        def trim_url(x):
            return left()
    else:
        def trim_url(x):
            return right()
    return trim_url
