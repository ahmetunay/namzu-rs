def ledger(rows):
    seen = set()
    totals = {}
    for identifier, account, amount in rows:
        if type(identifier) is not str or type(account) is not str or type(amount) is not int:
            raise TypeError("invalid ledger row")
        if identifier in seen:
            continue
        seen.add(identifier)
        totals[account] = totals.get(account, 0) + amount
    return dict(sorted(totals.items()))
