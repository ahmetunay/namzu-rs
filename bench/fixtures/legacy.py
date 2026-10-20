def normalize(text):
    if type(text) is not str:
        raise TypeError("text must be str")
    return " ".join(text.strip().lower().split())
