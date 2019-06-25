//! Subsetting of opentype fonts.

use std::collections::HashMap;
use std::io::{Cursor, Seek, SeekFrom};

use byteorder::{BE, ReadBytesExt, WriteBytesExt};
use opentype::{OpenTypeReader, Outlines, Table, TableRecord, Tag};
use opentype::tables::{Header, CharMap, Locations, HorizontalMetrics, Glyphs};

use crate::size::Size;
use super::{Font, FontError, FontResult};


/// Subsets a font.
#[derive(Debug)]
pub struct Subsetter<'a> {
    // The original font
    font: &'a Font,
    reader: OpenTypeReader<Cursor<&'a [u8]>>,
    outlines: Outlines,
    tables: Vec<TableRecord>,
    glyphs: Vec<u16>,

    // The subsetted font
    chars: Vec<char>,
    records: Vec<TableRecord>,
    body: Vec<u8>,
}

impl<'a> Subsetter<'a> {
    /// Subset a font. See [`Font::subetted`] for more details.
    pub fn subset<C, I, S>(font: &Font, chars: C, tables: I) -> Result<Font, FontError>
    where C: IntoIterator<Item=char>, I: IntoIterator<Item=S>, S: AsRef<str> {
        // Parse some header information.
        let mut reader = OpenTypeReader::from_slice(&font.program);
        let outlines = reader.outlines()?;
        let table_records = reader.tables()?.to_vec();

        // Store all chars we want in a vector.
        let chars: Vec<_> = chars.into_iter().collect();

        let subsetter = Subsetter {
            font,
            reader,
            outlines,
            tables: table_records,
            glyphs: Vec::with_capacity(1 + chars.len()),
            chars,
            records: vec![],
            body: vec![],
        };

        subsetter.run(tables)
    }

    /// Do the subsetting.
    fn run<I, S>(mut self, tables: I) -> FontResult<Font>
    where I: IntoIterator<Item=S>, S: AsRef<str> {
        if self.outlines == Outlines::CFF {
            return Err(FontError::UnsupportedFont("CFF outlines".to_string()));
        }

        // Find out which glyphs to include based on which characters we want and
        // which glyphs are additionally used by composite glyphs.
        self.find_glyphs()?;

        // Write all the tables the callee wants.
        for table in tables.into_iter() {
            let tag = table.as_ref().parse()
                .map_err(|_| FontError::UnsupportedTable(table.as_ref().to_string()))?;

            if self.contains_table(tag) {
                self.subset_table(tag)?;
            }
        }

        // Preprend the new header to the body. We have to do this last, because
        // we only have the necessary information now.
        self.write_header()?;

        Ok(Font {
            name: self.font.name.clone(),
            mapping: self.compute_mapping(),
            widths: self.compute_widths()?,
            program: self.body,
            default_glyph: self.font.default_glyph,
            metrics: self.font.metrics,
        })
    }

    /// Store all glyphs the subset shall contain into `self.glyphs`.
    fn find_glyphs(&mut self) -> FontResult<()> {
        if self.outlines == Outlines::TrueType {
            // Parse the necessary information.
            let char_map = self.read_table::<CharMap>()?;
            let glyf = self.read_table::<Glyphs>()?;

            // Add the default glyph at index 0 in any case.
            self.glyphs.push(self.font.default_glyph);

            // Add all the glyphs for the chars requested.
            for &c in &self.chars {
                let glyph = char_map.get(c).ok_or_else(|| FontError::MissingCharacter(c))?;
                self.glyphs.push(glyph);
            }

            // Collect the composite glyphs.
            let mut i = 0;
            while i < self.glyphs.len() as u16 {
                let glyph_id = self.glyphs[i as usize];
                let glyph = glyf.get(glyph_id).take_invalid("missing glyf entry")?;

                for &composite in &glyph.composites {
                    if self.glyphs.iter().rev().all(|&x| x != composite) {
                        self.glyphs.push(composite);
                    }
                }
                i += 1;
            }
        } else {
            unimplemented!()
        }

        Ok(())
    }

    /// Prepend the new header to the constructed body.
    fn write_header(&mut self) -> FontResult<()> {
        // Create an output buffer
        let header_len = 12 + self.records.len() * 16;
        let mut header = Vec::with_capacity(header_len);

        // Compute the first four header entries.
        let num_tables = self.records.len() as u16;

        // The highester power lower than the table count.
        let mut max_power = 1u16;
        while max_power * 2 <= num_tables {
            max_power *= 2;
        }
        max_power = std::cmp::min(max_power, num_tables);

        let search_range = max_power * 16;
        let entry_selector = (max_power as f32).log2() as u16;
        let range_shift = num_tables * 16 - search_range;

        // Write the base header
        header.write_u32::<BE>(match self.outlines {
            Outlines::TrueType => 0x00010000,
            Outlines::CFF => 0x4f54544f,
        })?;
        header.write_u16::<BE>(num_tables)?;
        header.write_u16::<BE>(search_range)?;
        header.write_u16::<BE>(entry_selector)?;
        header.write_u16::<BE>(range_shift)?;

        // Write the table records
        for record in &self.records {
            header.extend(record.tag.value());
            header.write_u32::<BE>(record.check_sum)?;
            header.write_u32::<BE>(header_len as u32 + record.offset)?;
            header.write_u32::<BE>(record.length)?;
        }

        // Prepend the fresh header to the body.
        header.append(&mut self.body);
        self.body = header;

        Ok(())
    }

    /// Compute the new widths.
    fn compute_widths(&self) -> FontResult<Vec<Size>> {
        let mut widths = Vec::with_capacity(self.glyphs.len());
        for &glyph in &self.glyphs {
            let &width = self.font.widths.get(glyph as usize)
                .take_invalid("missing glyph width")?;
            widths.push(width);
        }
        Ok(widths)
    }

    /// Compute the new mapping.
    fn compute_mapping(&self) -> HashMap<char, u16> {
        // The mapping is basically just the index in the char vector, but we add one
        // to each index here because we added the default glyph to the front.
        self.chars.iter().enumerate().map(|(i, &c)| (c, 1 + i as u16))
            .collect::<HashMap<char, u16>>()
    }

    /// Subset and write the table with the given tag to the output.
    fn subset_table(&mut self, tag: Tag) -> FontResult<()> {
        match tag.value() {
            // These tables can just be copied.
            b"head" | b"name" | b"OS/2" | b"post" |
            b"cvt " | b"fpgm" | b"prep" | b"gasp" => self.copy_table(tag),

            // These tables have more complex subsetting routines.
            b"hhea" => self.subset_hhea(),
            b"hmtx" => self.subset_hmtx(),
            b"maxp" => self.subset_maxp(),
            b"cmap" => self.subset_cmap(),
            b"glyf" => self.subset_glyf(),
            b"loca" => self.subset_loca(),

            _ => Err(FontError::UnsupportedTable(tag.to_string()))
        }
    }

    /// Copy the table body without modification.
    fn copy_table(&mut self, tag: Tag) -> FontResult<()> {
        self.write_table_body(tag, |this| {
            let table = this.read_table_data(tag)?;
            Ok(this.body.extend(table))
        })
    }

    /// Subset the `hhea` table by changing the number of horizontal metrics in it.
    fn subset_hhea(&mut self) -> FontResult<()> {
        let tag = "hhea".parse().unwrap();
        let hhea = self.read_table_data(tag)?;
        let glyph_count = self.glyphs.len() as u16;
        self.write_table_body(tag, |this| {
            this.body.extend(&hhea[..hhea.len() - 2]);
            this.body.write_u16::<BE>(glyph_count)?;
            Ok(())
        })
    }

    /// Subset the `hmtx` table by changing the included metrics.
    fn subset_hmtx(&mut self) -> FontResult<()> {
        let tag = "hmtx".parse().unwrap();
        let hmtx = self.read_table::<HorizontalMetrics>()?;
        self.write_table_body(tag, |this| {
            for &glyph in &this.glyphs {
                let metrics = hmtx.get(glyph).take_invalid("missing glyph metrics")?;
                this.body.write_u16::<BE>(metrics.advance_width)?;
                this.body.write_i16::<BE>(metrics.left_side_bearing)?;
            }
            Ok(())
        })
    }

    /// Subset the `maxp` table by changing the glyph count in it.
    fn subset_maxp(&mut self) -> FontResult<()> {
        let tag = "maxp".parse().unwrap();
        let maxp = self.read_table_data(tag)?;
        let glyph_count = self.glyphs.len() as u16;
        self.write_table_body(tag, |this| {
            this.body.extend(&maxp[..4]);
            this.body.write_u16::<BE>(glyph_count)?;
            Ok(this.body.extend(&maxp[6..]))
        })
    }

    /// Subset the `cmap` table by
    fn subset_cmap(&mut self) -> FontResult<()> {
        let tag = "cmap".parse().unwrap();

        // Always uses format 12 for simplicity.
        self.write_table_body(tag, |this| {
            let mut groups = Vec::new();

            // Find out which chars are in consecutive groups.
            let mut end = 0;
            let len = this.chars.len();
            while end < len {
                // Compute the end of the consecutive group.
                let start = end;
                while end + 1 < len && this.chars[end+1] as u32 == this.chars[end] as u32 + 1 {
                    end += 1;
                }

                // Add one to the start because we inserted the default glyph in front.
                let glyph_id = 1 + start;
                groups.push((this.chars[start], this.chars[end], glyph_id));
                end += 1;
            }

            // Write the table header.
            this.body.write_u16::<BE>(0)?;
            this.body.write_u16::<BE>(1)?;
            this.body.write_u16::<BE>(3)?;
            this.body.write_u16::<BE>(1)?;
            this.body.write_u32::<BE>(12)?;

            // Write the subtable header.
            this.body.write_u16::<BE>(12)?;
            this.body.write_u16::<BE>(0)?;
            this.body.write_u32::<BE>((16 + 12 * groups.len()) as u32)?;
            this.body.write_u32::<BE>(0)?;
            this.body.write_u32::<BE>(groups.len() as u32)?;

            // Write the subtable body.
            for group in &groups {
                this.body.write_u32::<BE>(group.0 as u32)?;
                this.body.write_u32::<BE>(group.1 as u32)?;
                this.body.write_u32::<BE>(group.2 as u32)?;
            }

            Ok(())
        })
    }

    /// Subset the `glyf` table by changing the indices of composite glyphs.
    fn subset_glyf(&mut self) -> FontResult<()> {
        let tag = "glyf".parse().unwrap();
        let loca = self.read_table::<Locations>()?;
        let glyf = self.read_table_data(tag)?;

        self.write_table_body(tag, |this| {
            for &glyph in &this.glyphs {
                // Find out the location of the glyph in the glyf table.
                let start = loca.offset(glyph).take_invalid("missing loca entry")?;
                let end = loca.offset(glyph + 1).take_invalid("missing loca entry")?;

                // If this glyph has no contours, skip it.
                if end == start {
                    continue;
                }

                // Extract the glyph data.
                let mut glyph_data = glyf.get(start as usize .. end as usize)
                    .take_invalid("missing glyph data")?.to_vec();

                // Construct a cursor to operate on the data.
                let mut cursor = Cursor::new(&mut glyph_data);
                let num_contours = cursor.read_i16::<BE>()?;

                // This is a composite glyph
                if num_contours < 0 {
                    cursor.seek(SeekFrom::Current(8))?;
                    loop {
                        let flags = cursor.read_u16::<BE>()?;

                        // Read the old glyph index.
                        let glyph_index = cursor.read_u16::<BE>()?;

                        // Compute the new glyph index by searching for it's index
                        // in the glyph vector.
                        let new_glyph_index = this.glyphs.iter()
                            .position(|&g| g == glyph_index)
                            .take_invalid("invalid composite glyph")? as u16;

                        // Overwrite the old index with the new one.
                        cursor.seek(SeekFrom::Current(-2))?;
                        cursor.write_u16::<BE>(new_glyph_index)?;

                        // This was the last component
                        if flags & 0x0020 == 0 {
                            break;
                        }

                        // Skip additional arguments.
                        let skip = if flags & 1 != 0 { 4 } else { 2 }
                            + if flags & 8 != 0 { 2 }
                            else if flags & 64 != 0 { 4 }
                            else if flags & 128 != 0 { 8 }
                            else { 0 };

                        cursor.seek(SeekFrom::Current(skip))?;
                    }
                }

                this.body.extend(glyph_data);
            }
            Ok(())
        })
    }

    /// Subset the `loca` table by changing to the new offsets.
    fn subset_loca(&mut self) -> FontResult<()> {
        let format = self.read_table::<Header>()?.index_to_loc_format;
        let tag = "loca".parse().unwrap();
        let loca = self.read_table::<Locations>()?;

        self.write_table_body(tag, |this| {
            let mut offset = 0;
            for &glyph in &this.glyphs {
                if format == 0 {
                    this.body.write_u16::<BE>((offset / 2) as u16)?;
                } else {
                    this.body.write_u32::<BE>(offset)?;
                }

                let len = loca.length(glyph).take_invalid("missing loca entry")?;
                offset += len;
            }
            this.body.write_u32::<BE>(offset)?;
            Ok(())
        })
    }

    /// Let a writer write the table body and then store the relevant metadata.
    fn write_table_body<F>(&mut self, tag: Tag, writer: F) -> FontResult<()>
    where F: FnOnce(&mut Self) -> FontResult<()> {
        // Run the writer and capture the length.
        let start = self.body.len();
        writer(self)?;
        let end = self.body.len();

        // Pad with zeroes.
        while (self.body.len() - start) % 4 != 0 {
            self.body.push(0);
        }

        Ok(self.records.push(TableRecord {
            tag,
            check_sum: calculate_check_sum(&self.body[start..]),
            offset: start as u32,
            length: (end - start) as u32,
        }))
    }

    /// Read a table with the opentype reader.
    fn read_table<T: Table>(&mut self) -> FontResult<T> {
        self.reader.read_table::<T>().map_err(Into::into)
    }

    /// Read the raw table data of a table.
    fn read_table_data(&self, tag: Tag) -> FontResult<&'a [u8]> {
        let record = match self.tables.binary_search_by_key(&tag, |r| r.tag) {
            Ok(index) => &self.tables[index],
            Err(_) => return Err(FontError::MissingTable(tag.to_string())),
        };

        self.font.program
            .get(record.offset as usize .. (record.offset + record.length) as usize)
            .take_invalid("missing table data")
    }

    /// Whether this font contains a given table.
    fn contains_table(&self, tag: Tag) -> bool {
        self.tables.binary_search_by_key(&tag, |r| r.tag).is_ok()
    }
}

/// Calculate a checksum over the sliced data as sum of u32's. The data length has to be a multiple
/// of four.
fn calculate_check_sum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    data.chunks_exact(4).for_each(|c| {
        sum = sum.wrapping_add(
            ((c[0] as u32) << 24)
          + ((c[1] as u32) << 16)
          + ((c[2] as u32) << 8)
          + (c[3] as u32)
        );
    });
    sum
}

/// Helper trait to create subsetting errors more easily.
trait TakeInvalid<T>: Sized {
    /// Pull the type out of the option, returning an invalid font error if self was not valid.
    fn take_invalid<S: Into<String>>(self, message: S) -> FontResult<T>;
}

impl<T> TakeInvalid<T> for Option<T> {
    fn take_invalid<S: Into<String>>(self, message: S) -> FontResult<T> {
        self.ok_or(FontError::InvalidFont(message.into()))
    }
}


#[cfg(test)]
mod tests {
    use crate::font::Font;

    #[test]
    fn subset() {
        let program = std::fs::read("../fonts/SourceSansPro-Regular.ttf").unwrap();
        let font = Font::new(program).unwrap();

        let subsetted = font.subsetted(
            "abcdefghijklmnopqrstuvwxyz‼".chars(),
            &["name", "OS/2", "post", "head", "hhea", "hmtx", "maxp", "cmap",
              "cvt ", "fpgm", "prep", "loca", "glyf"][..]
        ).unwrap();

        std::fs::write("../target/SourceSansPro-Subsetted.ttf", &subsetted.program).unwrap();
    }
}