use super::*;

#[derive(Debug, Clone, Serialize)]
pub struct OpenClosed<O: ToOrder> {
    pub r: usize,
    /// Do open syncmers first?
    pub open: bool,
    /// Do closed syncmers second?
    pub closed: bool,
    /// When true, open syncmers must have this offset instead of in the middle by default.
    pub offset: Option<usize>,
    /// When true, any position offset%w makes a kmer an open syncmer.
    pub modulo: bool,
    // FIXME: tiebreak by reverse tmer.
    pub open_by_tmer: bool,
    pub closed_by_tmer: bool,
    pub other_by_tmer: bool,
    pub anti_tmer: bool,
    pub o: O,
}

impl OpenClosed<RandomO> {
    pub fn default(r: usize) -> Self {
        Self {
            r,
            open: false,
            closed: false,
            open_by_tmer: false,
            closed_by_tmer: false,
            other_by_tmer: false,
            offset: None,
            modulo: false,
            anti_tmer: false,
            o: RandomO,
        }
    }
}

impl<OO: Order<T = usize>, O: ToOrder<O = OO>> ToOrder for OpenClosed<O> {
    type O = OpenClosedO<O::O>;
    fn to_order(&self, w: usize, k: usize, sigma: usize) -> Self::O {
        let Self {
            r,
            open,
            closed,
            open_by_tmer,
            closed_by_tmer,
            other_by_tmer,
            offset,
            modulo,
            anti_tmer,
            ..
        }: OpenClosed<O> = *self;
        let offset = if modulo {
            offset.unwrap_or((k - r) % w / 2) % w
        } else {
            offset.unwrap_or((k - r) / 2)
        };
        OpenClosedO {
            r,
            w,
            k,
            open,
            closed,
            open_by_tmer,
            closed_by_tmer,
            other_by_tmer,
            offset,
            modulo,
            anti_tmer,
            m: M::build_from_order(&self.o, k - r + 1, r, sigma),
        }
    }
}

pub struct OpenClosedO<O: Order> {
    r: usize,
    w: usize,
    k: usize,
    open: bool,
    closed: bool,
    open_by_tmer: bool,
    closed_by_tmer: bool,
    other_by_tmer: bool,
    offset: usize,
    modulo: bool,
    anti_tmer: bool,
    m: Minimizer<O>,
}

impl<O: Order<T = usize>> OpenClosedO<O> {
    #[inline(always)]
    fn inner_key(&self, kmer: &[u8], x: usize) -> (u8, O::T) {
        let w0 = self.k - self.r;
        assert!(x <= w0);
        let p;
        let by_tmer;
        // FIXME: Re-use fastreduce.
        let is_open = if self.modulo { x % self.w } else { x } == self.offset;
        let is_closed = x == 0 || x == w0;
        if self.open && is_open {
            p = 0;
            by_tmer = self.open_by_tmer;
        } else if self.closed && is_closed {
            p = 1;
            by_tmer = self.closed_by_tmer;
        } else {
            p = 2;
            by_tmer = self.other_by_tmer;
        }
        let tiebreak = if by_tmer {
            self.m.ord().key(&kmer[x..x + self.r])
        } else {
            if self.anti_tmer {
                !self.m.ord().key(&kmer[x..x + self.r])
            } else {
                O::T::default()
            }
        };
        (p, tiebreak)
    }
}

impl<O: Order<T = usize>> Order for OpenClosedO<O> {
    type T = (u8, O::T);

    #[inline(always)]
    fn key(&self, kmer: &[u8]) -> Self::T {
        assert_eq!(kmer.len(), self.k);
        let x = self.m.sample(kmer);
        self.inner_key(kmer, x)
    }

    #[inline(always)]
    fn keys(&self, text: &[u8], k: usize) -> impl Iterator<Item = Self::T> {
        assert_eq!(k, self.k);
        self.m
            .stream(text)
            .into_iter()
            .zip(text.windows(k))
            .map(move |(x, kmer)| self.inner_key(kmer, x))
    }
}