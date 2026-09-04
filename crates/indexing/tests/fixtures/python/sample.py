"""Module docstring."""

def free_function():
    x = 1
    return x


def documented(name: str) -> str:
    """Returns a greeting for the given name."""
    return f"hello {name}"


class Tracker:
    def __init__(self):
        self.value = 0

    def update(self):
        self.value += 1


def test_it_works():
    assert True
