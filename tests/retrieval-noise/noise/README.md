# Coverage noise experiment

Separate fictional fixtures; the frozen RU/EN baseline is unchanged. Compare
natural with no filter and thresholds 0, 0.5, 0.6 and 1. Common-word matches and
partial negative queries should be removed above one-third coverage. The complete
lexical collision deliberately remains a negative violation at every threshold;
coverage cannot establish semantic relevance. Inspect returned rank 1 as well as
all returned documents; a result limit cannot repair a wrong first result.
