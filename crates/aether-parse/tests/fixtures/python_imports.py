import os
import pkg.alpha as alpha, pkg.beta
from core.util import helper
from .local import thing
from ..shared import model
from pkg.star import *


def execute(value: int) -> str:
    result = helper(value)
    return thing.format(str(result))
