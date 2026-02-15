from typing import Iterable

MODULE_FLAG: bool = True
type UserId = int


def top_level(values: Iterable[int]) -> int:
    def nested(inner: int) -> int:
        return inner + 1

    return nested(sum(values))


@decorator
def decorated_function(name: str) -> str:
    return name.upper()


@decorator
class Worker:
    def __init__(self, name: str) -> None:
        self.name = name

    @staticmethod
    def factory(value: str) -> "Worker":
        return Worker(value)

    def run(self) -> str:
        return decorated_function(self.name)
