#!/usr/bin/env bash
# Usage: eval/run.sh [path/to/wica]
# For each query, reports the rank of the first acceptable tool in `wica --all`.
W=${1:-target/release/wica}
total=0; hit1=0; hit5=0; mrr=0
while IFS='|' read -r q expected; do
  [[ -z "$q" || "$q" == \#* ]] && continue
  q=$(echo "$q" | xargs)
  mapfile -t names < <("$W" --all -q -n 10 $q 2>/dev/null | sed -E 's/ \(not installed\)$//' | awk 'match($0,/^ [^ ].*/){print}' | sed -E 's/^ //; s/  .*//; s/ +$//')
  rank=0
  for i in "${!names[@]}"; do
    IFS=',' read -ra exp <<< "$expected"
    for e in "${exp[@]}"; do
      [[ "${names[$i]}" == "$(echo "$e" | xargs)" ]] && { rank=$((i+1)); break 2; }
    done
  done
  total=$((total+1))
  ((rank==1)) && hit1=$((hit1+1))
  ((rank>=1 && rank<=5)) && hit5=$((hit5+1))
  ((rank>0)) && mrr=$(awk -v m="$mrr" -v r="$rank" 'BEGIN{print m + 1/r}')
  printf '%-34s %-6s top3: %s\n' "$q" "$( ((rank)) && echo "#$rank" || echo MISS)" "$(IFS=,; echo "${names[*]:0:3}")"
done < "${2:-eval/queries.txt}"
printf '\ntop1: %d/%d  top5: %d/%d  MRR: %.3f\n' $hit1 $total $hit5 $total "$(awk -v m="$mrr" -v t="$total" 'BEGIN{print m / t}')"
