import legacy


def normalize_many(texts):
    return [legacy.normalize(text) for text in texts]
