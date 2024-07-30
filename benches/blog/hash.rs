use std::array::from_fn;

pub trait Hasher {
    type Out;
    fn hash(&self, t: &[u8]) -> Self::Out;
    #[inline(always)]
    fn hash_kmers(&self, k: usize, t: &[u8]) -> impl Iterator<Item = Self::Out> {
        t.windows(k).map(|kmer| self.hash(kmer))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FxHash;
impl Hasher for FxHash {
    type Out = u64;
    fn hash(&self, t: &[u8]) -> u64 {
        fxhash::hash64(t)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ExtNtHash;
impl Hasher for ExtNtHash {
    type Out = u64;
    fn hash(&self, t: &[u8]) -> u64 {
        nthash::ntf64(t, 0, t.len())
    }
    fn hash_kmers(&self, k: usize, t: &[u8]) -> impl Iterator<Item = Self::Out> {
        nthash::NtHashForwardIterator::new(t, k).unwrap()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct WyHash;
impl Hasher for WyHash {
    type Out = u64;
    fn hash(&self, t: &[u8]) -> u64 {
        wyhash::wyhash(t, 0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Buffer<H> {
    pub hasher: H,
}
impl<H: Hasher> Hasher for Buffer<H> {
    type Out = H::Out;
    fn hash(&self, t: &[u8]) -> Self::Out {
        self.hasher.hash(t)
    }
    fn hash_kmers(&self, k: usize, t: &[u8]) -> impl Iterator<Item = Self::Out> {
        self.hasher.hash_kmers(k, t).collect::<Vec<_>>().into_iter()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Buffer2<H> {
    pub hasher: H,
}
impl<H: Hasher<Out: Default + Clone>> Hasher for Buffer2<H> {
    type Out = H::Out;
    fn hash(&self, t: &[u8]) -> Self::Out {
        self.hasher.hash(t)
    }

    fn hash_kmers(&self, k: usize, t: &[u8]) -> impl Iterator<Item = Self::Out> {
        let len = t.len() - k + 1;
        let mut v = vec![H::Out::default(); len];
        // assert_eq!(v.len(), len);
        let mut it = self.hasher.hash_kmers(k, t);
        // for x in v.iter_mut() {
        //     *x = it.next().unwrap();
        // }
        for i in 0..len {
            unsafe { v.as_mut_ptr().add(i).write(it.next().unwrap()) };
        }
        v.into_iter()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BufferDouble<H> {
    pub hasher: H,
}
impl<H: Hasher<Out: Default + Clone>> Hasher for BufferDouble<H> {
    type Out = H::Out;
    fn hash(&self, t: &[u8]) -> Self::Out {
        self.hasher.hash(t)
    }

    fn hash_kmers(&self, k: usize, t: &[u8]) -> impl Iterator<Item = Self::Out> {
        let num_kmers = t.len() - k + 1;
        // For odd num_kmers, we skip the last kmer.
        let kmers_per_part = num_kmers / 2;
        let part_len = kmers_per_part + k - 1;
        let t0 = &t[..part_len];
        let t1 = &t[kmers_per_part..kmers_per_part + part_len];
        let mut v = vec![H::Out::default(); 2 * kmers_per_part];
        let mut it0 = self.hasher.hash_kmers(k, t0);
        let mut it1 = self.hasher.hash_kmers(k, t1);
        for i in 0..kmers_per_part {
            unsafe {
                v.as_mut_ptr().add(i).write(it0.next().unwrap());
                v.as_mut_ptr()
                    .add(kmers_per_part + i)
                    .write(it1.next().unwrap());
            }
        }
        v.into_iter()
    }
}
