"""Optical pair adjustments in font units; advances remain the spacing default."""

# The approved title's spacing is preserved exactly.
TITLE_KERN = {
    ("s", "p"): -10, ("p", "l"): -12, ("l", "i"): -12,
    ("i", "n"): -12, ("n", "t"): -12, ("t", "e"): -8,
    ("e", "r"): -8, ("r", "m"): -10,
}


def pairs():
    result = {}

    def group(lefts, rights, value):
        for left in lefts:
            for right in rights:
                result[left, right] = value

    # Cap diagonals, rounds, and overhanging bars.
    group("A", "TVWY", -55)
    group("A", "CGOQU", -18)
    group("TVWY", "A", -55)
    group("VY", "CGOQ", -24)
    group("CGOQ", "ATVXY", -22)
    group("L", "TVWY", -66)
    group("L", "CGOQ", -24)
    group("FP", "A", -45)
    group("K", "CGOQ", -27)
    group("R", "TVWY", -18)
    group("T", "CGOQ", -18)
    group("T", "aceosuvwy", -48)
    group("VY", "aceos", -46)
    group("W", "aceos", -29)
    group("F", "aceos", -20)
    group("P", "aeo", -16)
    group("K", "aeou", -15)
    group("L", "vwy", -30)

    # Lowercase wedges against bowls; straight-stem neighbors retain sidebearings.
    group("vwy", "aceo", -18)
    group("aceo", "vwy", -18)
    group("r", "aceo", -14)
    group("f", "aceo", -10)
    group("k", "aceo", -15)
    group("t", "ao", -8)
    group("bop", "x", -14)
    group("x", "ceo", -14)

    # Punctuation and quotes need optical proximity without compressing figures.
    group("TVWYFPrvwy", ".,", -52)
    group(".", "TVWY", -45)
    group("A", "’”'\"", -43)
    group("‘“'\"", "A", -43)
    group("-–—", "TVWY", -24)
    group("TVWY", "-–—", -24)
    group("(", "CGOQ", -14)
    group("CGOQ", ")", -14)

    result.update(TITLE_KERN)
    return result


KERN = pairs()
