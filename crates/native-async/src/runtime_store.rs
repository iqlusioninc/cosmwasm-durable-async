//! Host cache and ordered indexes. Iterators remain lazy over the backing store.
use cosmwasm_std::{from_json, to_json_vec, Order, Record, StdError, StdResult, Storage};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::BTreeMap;

pub(crate) struct Cache<'a> {
    base: &'a mut dyn Storage,
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
}
impl<'a> Cache<'a> {
    pub fn new(base: &'a mut dyn Storage) -> Self {
        Self {
            base,
            writes: BTreeMap::new(),
        }
    }
    pub fn commit(self) {
        for (key, value) in self.writes {
            match value {
                Some(value) => self.base.set(&key, &value),
                None => self.base.remove(&key),
            }
        }
    }
}
impl Storage for Cache<'_> {
    fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        self.writes
            .get(key)
            .cloned()
            .unwrap_or_else(|| self.base.get(key))
    }
    fn set(&mut self, key: &[u8], value: &[u8]) {
        self.writes.insert(key.to_vec(), Some(value.to_vec()));
    }
    fn remove(&mut self, key: &[u8]) {
        self.writes.insert(key.to_vec(), None);
    }
    fn range<'a>(
        &'a self,
        start: Option<&[u8]>,
        end: Option<&[u8]>,
        order: Order,
    ) -> Box<dyn Iterator<Item = Record> + 'a> {
        let changed: Vec<_> = self
            .writes
            .iter()
            .filter(|(key, _)| {
                start.is_none_or(|s| key.as_slice() >= s) && end.is_none_or(|e| key.as_slice() < e)
            })
            .filter_map(|(k, v)| v.clone().map(|v| (k.clone(), v)))
            .collect();
        let mut changed = if order == Order::Ascending {
            changed
        } else {
            changed.into_iter().rev().collect()
        }
        .into_iter()
        .peekable();
        let mut base = self
            .base
            .range(start, end, order)
            .filter(|(k, _)| !self.writes.contains_key(k))
            .peekable();
        Box::new(std::iter::from_fn(move || {
            match (base.peek(), changed.peek()) {
                (Some(a), Some(b)) => {
                    if (order == Order::Ascending && a.0 < b.0)
                        || (order == Order::Descending && a.0 > b.0)
                    {
                        base.next()
                    } else {
                        changed.next()
                    }
                }
                (Some(_), None) => base.next(),
                (None, Some(_)) => changed.next(),
                _ => None,
            }
        }))
    }
}
pub(crate) fn get<T: DeserializeOwned>(s: &dyn Storage, k: &[u8]) -> StdResult<T> {
    from_json(
        s.get(k)
            .ok_or_else(|| StdError::generic_err("missing runtime record"))?,
    )
}
pub(crate) fn put<T: Serialize>(s: &mut dyn Storage, k: &[u8], v: &T) -> StdResult<()> {
    s.set(k, &to_json_vec(v)?);
    Ok(())
}
pub(crate) fn number(s: &dyn Storage, k: &[u8]) -> StdResult<u64> {
    s.get(k).map(from_json).transpose().map(|v| v.unwrap_or(0))
}
pub(crate) fn bump(s: &mut dyn Storage, k: &[u8]) -> StdResult<u64> {
    let n = number(s, k)?
        .checked_add(1)
        .ok_or_else(|| StdError::generic_err("runtime counter exhausted"))?;
    put(s, k, &n)?;
    Ok(n)
}
pub(crate) fn first(s: &dyn Storage, prefix: &[u8]) -> Option<Record> {
    let mut end = prefix.to_vec();
    *end.last_mut().unwrap() += 1;
    s.range(Some(prefix), Some(&end), Order::Ascending).next()
}
