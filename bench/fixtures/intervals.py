def merge_intervals(intervals):
    ordered = []
    for start, end in intervals:
        if type(start) is not int or type(end) is not int:
            raise TypeError("integer endpoints required")
        if start > end:
            raise ValueError("reversed interval")
        if start < end:
            ordered.append((start, end))
    result = []
    for start, end in sorted(ordered):
        if result and start <= result[-1][1]:
            result[-1] = (result[-1][0], max(result[-1][1], end))
        else:
            result.append((start, end))
    return result
