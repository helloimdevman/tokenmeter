# JSON structure only: keys and value types (numbers shown, strings redacted). Imported by grok_probe.py, grok_sub.py.
# usage: from shape import shape
import json, sys
def shape(v, depth=0, maxd=8):
    if isinstance(v, dict):
        if depth >= maxd: return "{...}"
        return {k: shape(x, depth+1, maxd) for k, x in v.items()}
    if isinstance(v, list):
        return [shape(v[0], depth+1, maxd)] + (["...%d" % len(v)] if len(v) > 1 else []) if v else []
    if isinstance(v, str): return "<str>"
    return v  # numbers/bool/null are fine
