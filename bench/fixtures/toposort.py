import heapq


def plan(nodes, edges):
    nodes = set(nodes)
    children = {node: set() for node in nodes}
    incoming = dict.fromkeys(nodes, 0)
    for source, target in edges:
        if source not in nodes or target not in nodes:
            raise ValueError("unknown node")
        if target not in children[source]:
            children[source].add(target)
            incoming[target] += 1
    ready = [node for node in nodes if incoming[node] == 0]
    heapq.heapify(ready)
    result = []
    while ready:
        node = heapq.heappop(ready)
        result.append(node)
        for child in children[node]:
            incoming[child] -= 1
            if incoming[child] == 0:
                heapq.heappush(ready, child)
    if len(result) != len(nodes):
        raise ValueError("cycle")
    return result
