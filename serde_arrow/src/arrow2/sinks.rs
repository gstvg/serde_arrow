use arrow2::{
    array::Array,
    array::{ListArray, MapArray, MutableUtf8Array, NullArray, StructArray, UnionArray, Utf8Array},
    bitmap::Bitmap,
    buffer::Buffer,
    datatypes::{DataType, Field, UnionMode},
    types::{f16, Offset},
};

use crate::{
    base::error::fail,
    generic::{
        chrono::{NaiveDateTimeStrBuilder, UtcDateTimeStrBuilder},
        schema::{Strategy, STRATEGY_KEY},
        sinks::{
            ArrayBuilder, ArraysBuilder, DynamicArrayBuilder, ListArrayBuilder, MapArrayBuilder,
            StructArrayBuilder, StructArrayBuilderState, TupleArrayBuilderState,
            TupleStructBuilder, UnionArrayBuilder,
        },
    },
    Result,
};

use arrow2::{
    array::{BooleanArray, MutableBooleanArray, MutablePrimitiveArray, PrimitiveArray},
    types::NativeType,
};

use crate::{
    base::{Event, EventSink},
    Error,
};

use super::schema::check_strategy;

type Arrow2ArrayBuilder = DynamicArrayBuilder<Box<dyn Array>>;

pub fn build_arrays_builder(
    fields: &[Field],
) -> Result<ArraysBuilder<Arrow2ArrayBuilder, Box<dyn Array>>> {
    let mut columns = Vec::new();
    let mut builders = Vec::new();

    for field in fields {
        builders.push(build_array_builder(field)?);
        columns.push(field.name.to_owned());
    }

    ArraysBuilder::new(columns, builders)
}

pub fn build_array_builder(field: &Field) -> Result<Arrow2ArrayBuilder> {
    check_strategy(field)?;

    match field.data_type() {
        DataType::Null => Ok(DynamicArrayBuilder::new(NullArrayBuilder::new())),
        DataType::Boolean => Ok(DynamicArrayBuilder::new(BooleanArrayBuilder::new())),
        DataType::Int8 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<i8>::new())),
        DataType::Int16 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<i16>::new())),
        DataType::Int32 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<i32>::new())),
        DataType::Int64 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<i64>::new())),
        DataType::UInt8 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<u8>::new())),
        DataType::UInt16 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<u16>::new())),
        DataType::UInt32 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<u32>::new())),
        DataType::UInt64 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<u64>::new())),
        DataType::Float16 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<f16>::new())),
        DataType::Float32 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<f32>::new())),
        DataType::Float64 => Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<f64>::new())),
        DataType::Utf8 => Ok(DynamicArrayBuilder::new(Utf8ArrayBuilder::<i32>::new())),
        DataType::LargeUtf8 => Ok(DynamicArrayBuilder::new(Utf8ArrayBuilder::<i64>::new())),
        DataType::Date64 => {
            if let Some(strategy) = field.metadata.get(STRATEGY_KEY) {
                let strategy: Strategy = strategy.parse()?;
                match strategy {
                    Strategy::NaiveDateTimeStr => Ok(DynamicArrayBuilder::new(
                        NaiveDateTimeStrBuilder(PrimitiveArrayBuilder::<i64>::new()),
                    )),
                    Strategy::UtcDateTimeStr => Ok(DynamicArrayBuilder::new(
                        UtcDateTimeStrBuilder(PrimitiveArrayBuilder::<i64>::new()),
                    )),
                    s => fail!("Invalid strategy {s} for Date64 column"),
                }
            } else {
                // TODO: is this correct?
                Ok(DynamicArrayBuilder::new(PrimitiveArrayBuilder::<i64>::new()))
            }
        }
        DataType::Struct(fields) => {
            let mut columns = Vec::new();
            let mut builders = Vec::new();
            let mut nullable = Vec::new();

            for field in fields {
                columns.push(field.name.to_owned());
                builders.push(build_array_builder(field)?);
                nullable.push(field.is_nullable);
            }

            if let Some(strategy) = field.metadata.get(STRATEGY_KEY) {
                let strategy: Strategy = strategy.parse()?;
                if !matches!(strategy, Strategy::Tuple) {
                    fail!("Invalid strategy {strategy} for Struct column");
                }
                let builder = TupleStructBuilder::new(nullable, builders);
                Ok(DynamicArrayBuilder::new(builder))
            } else {
                let builder = StructArrayBuilder::new(columns, nullable, builders);
                Ok(DynamicArrayBuilder::new(builder))
            }
        }
        // TODO: test List sink
        DataType::List(field) => {
            let values = build_array_builder(field.as_ref())?;
            let builder =
                ListArrayBuilder::<_, i32>::new(values, field.name.to_owned(), field.is_nullable);
            Ok(DynamicArrayBuilder::new(builder))
        }
        DataType::LargeList(field) => {
            let values = build_array_builder(field.as_ref())?;
            let builder =
                ListArrayBuilder::<_, i64>::new(values, field.name.to_owned(), field.is_nullable);
            Ok(DynamicArrayBuilder::new(builder))
        }
        DataType::Union(fields, field_indices, mode) => {
            if field_indices.is_some() {
                fail!("Union types with explicit field indices are not supported");
            }
            if !mode.is_dense() {
                fail!("Only dense unions are supported at the moment");
            }

            let mut field_builders = Vec::new();
            let mut field_nullable = Vec::new();

            for field in fields {
                field_builders.push(build_array_builder(field)?);
                field_nullable.push(field.is_nullable);
            }

            let builder = UnionArrayBuilder::new(field_builders, field_nullable, field.is_nullable);
            Ok(DynamicArrayBuilder::new(builder))
        }
        DataType::Map(field, _) => {
            let kv_fields = match field.data_type() {
                DataType::Struct(fields) => fields,
                dt => fail!("Expected inner field of Map to be Struct, found: {dt:?}"),
            };
            if kv_fields.len() != 2 {
                fail!(
                    "Expected two fields (key and value) in map struct, found: {}",
                    kv_fields.len()
                );
            }

            let key_builder = build_array_builder(&kv_fields[0])?;
            let val_builder = build_array_builder(&kv_fields[1])?;

            let builder = MapArrayBuilder::new(key_builder, val_builder, field.is_nullable);
            Ok(DynamicArrayBuilder::new(builder))
        }
        _ => fail!(
            "Cannot build sink for {} with type {:?}",
            field.name,
            field.data_type
        ),
    }
}

impl<B: ArrayBuilder<Box<dyn Array>>> ArrayBuilder<Box<dyn Array>> for StructArrayBuilder<B> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        if !matches!(self.state, StructArrayBuilderState::Start) {
            fail!("Invalid state at array construction");
        }

        let values: Result<Vec<Box<dyn Array>>> =
            self.builders.into_iter().map(|b| b.into_array()).collect();
        let values = values?;

        let mut fields = Vec::new();
        for (i, column) in self.columns.into_iter().enumerate() {
            fields.push(Field::new(
                column,
                values[i].data_type().clone(),
                self.nullable[i],
            ));
        }
        let data_type = DataType::Struct(fields);

        Ok(Box::new(StructArray::new(
            data_type,
            values,
            Some(self.validity.into()),
        )))
    }
}

impl<B: ArrayBuilder<Box<dyn Array>>> ArrayBuilder<Box<dyn Array>> for TupleStructBuilder<B> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        if !matches!(self.state, TupleArrayBuilderState::Start) {
            fail!("Invalid state at array construction");
        }

        let values: Result<Vec<Box<dyn Array>>> =
            self.builders.into_iter().map(|b| b.into_array()).collect();
        let values = values?;

        let mut fields = Vec::new();
        for (i, value) in values.iter().enumerate() {
            fields.push(Field::new(
                i.to_string(),
                value.data_type().clone(),
                self.nullable[i],
            ));
        }
        let data_type = DataType::Struct(fields);

        Ok(Box::new(StructArray::new(
            data_type,
            values,
            Some(self.validity.into()),
        )))
    }
}

impl<B: ArrayBuilder<Box<dyn Array>>> ArrayBuilder<Box<dyn Array>> for UnionArrayBuilder<B> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        let values: Result<Vec<Box<dyn Array>>> = self
            .field_builders
            .into_iter()
            .map(|b| b.into_array())
            .collect();
        let values = values?;

        let mut fields = Vec::new();
        for (i, value) in values.iter().enumerate() {
            fields.push(Field::new(
                i.to_string(),
                value.data_type().clone(),
                self.field_nullable[i],
            ));
        }
        let data_type = DataType::Union(fields, None, UnionMode::Dense);

        Ok(Box::new(UnionArray::new(
            data_type,
            self.field_types.into(),
            values,
            Some(self.field_offsets.into()),
        )))
    }
}

#[derive(Debug, Default)]
pub struct NullArrayBuilder {
    length: usize,
}

impl NullArrayBuilder {
    pub fn new() -> Self {
        Self { length: 0 }
    }
}

impl EventSink for NullArrayBuilder {
    fn accept_some(&mut self) -> Result<()> {
        Ok(())
    }

    fn accept_null(&mut self) -> Result<()> {
        self.length += 1;
        Ok(())
    }

    fn accept_default(&mut self) -> Result<()> {
        self.length += 1;
        Ok(())
    }

    fn accept(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Some => self.accept_some(),
            Event::Null => self.accept_null(),
            Event::Default => self.accept_default(),
            ev => fail!("NullArrayBuilder cannot accept event {ev}"),
        }
    }
}

impl ArrayBuilder<Box<dyn Array>> for NullArrayBuilder {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        Ok(Box::new(NullArray::new(DataType::Null, self.length)))
    }
}

#[derive(Debug, Default)]
pub struct PrimitiveArrayBuilder<T: NativeType + for<'a> TryFrom<Event<'a>, Error = Error>> {
    array: MutablePrimitiveArray<T>,
}

impl<T: NativeType + for<'a> TryFrom<Event<'a>, Error = Error>> PrimitiveArrayBuilder<T> {
    pub fn new() -> Self {
        Self::default()
    }
}

macro_rules! impl_primitive_array_builder {
    ($ty:ty, $func:ident, $variant:ident) => {
        impl EventSink for PrimitiveArrayBuilder<$ty> {
            fn $func(&mut self, val: $ty) -> Result<()> {
                self.array.push(Some(val));
                Ok(())
            }

            fn accept_default(&mut self) -> Result<()> {
                self.array.push(Some(Default::default()));
                Ok(())
            }

            fn accept_null(&mut self) -> Result<()> {
                self.array.push(None);
                Ok(())
            }

            fn accept_some(&mut self) -> Result<()> {
                Ok(())
            }

            fn accept(&mut self, event: Event<'_>) -> Result<()> {
                match event {
                    Event::Some => self.accept_some(),
                    Event::Default => self.accept_default(),
                    Event::Null => self.accept_null(),
                    Event::$variant(val) => self.$func(val),
                    ev => fail!(
                        "Cannot handle event {ev} in PrimitiveArrayBuilder<{}>",
                        stringify!($ty)
                    ),
                }
            }
        }

        impl ArrayBuilder<Box<dyn Array>> for PrimitiveArrayBuilder<$ty> {
            fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
                (*self).into_array()
            }

            fn into_array(self) -> Result<Box<dyn Array>>
            where
                Self: Sized,
            {
                Ok(Box::new(PrimitiveArray::from(self.array)))
            }
        }
    };
}

impl_primitive_array_builder!(i8, accept_i8, I8);
impl_primitive_array_builder!(i16, accept_i16, I16);
impl_primitive_array_builder!(i32, accept_i32, I32);
impl_primitive_array_builder!(i64, accept_i64, I64);

impl_primitive_array_builder!(u8, accept_u8, U8);
impl_primitive_array_builder!(u16, accept_u16, U16);
impl_primitive_array_builder!(u32, accept_u32, U32);
impl_primitive_array_builder!(u64, accept_u64, U64);

impl_primitive_array_builder!(f32, accept_f32, F32);
impl_primitive_array_builder!(f64, accept_f64, F64);

impl EventSink for PrimitiveArrayBuilder<f16> {
    fn accept_f32(&mut self, val: f32) -> Result<()> {
        self.array.push(Some(f16::from_f32(val)));
        Ok(())
    }

    fn accept_default(&mut self) -> Result<()> {
        self.array.push(Some(Default::default()));
        Ok(())
    }

    fn accept_null(&mut self) -> Result<()> {
        self.array.push(None);
        Ok(())
    }

    fn accept_some(&mut self) -> Result<()> {
        Ok(())
    }

    fn accept(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Some => self.accept_some(),
            Event::Default => self.accept_default(),
            Event::Null => self.accept_null(),
            Event::F32(val) => self.accept_f32(val),
            ev => fail!("Cannot handle event {ev} in PrimitiveArrayBuilder<f16>"),
        }
    }
}

impl ArrayBuilder<Box<dyn Array>> for PrimitiveArrayBuilder<f16> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        Ok(Box::new(PrimitiveArray::from(self.array)))
    }
}

#[derive(Debug, Default)]
pub struct BooleanArrayBuilder {
    array: MutableBooleanArray,
}

impl BooleanArrayBuilder {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EventSink for BooleanArrayBuilder {
    fn accept_bool(&mut self, val: bool) -> Result<()> {
        self.array.push(Some(val));
        Ok(())
    }

    fn accept_default(&mut self) -> Result<()> {
        self.array.push(Some(false));
        Ok(())
    }

    fn accept_null(&mut self) -> Result<()> {
        self.array.push(None);
        Ok(())
    }

    fn accept_some(&mut self) -> Result<()> {
        Ok(())
    }

    fn accept(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Some => self.accept_some(),
            Event::Default => self.accept_default(),
            Event::Null => self.accept_null(),
            Event::Bool(val) => self.accept_bool(val),
            ev => fail!("Cannot handle event {ev} in BooleanArrayBuilder"),
        }
    }
}

impl ArrayBuilder<Box<dyn Array>> for BooleanArrayBuilder {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>> {
        Ok(Box::new(BooleanArray::from(self.array)))
    }
}

#[derive(Debug, Default)]
pub struct Utf8ArrayBuilder<O: Offset> {
    array: MutableUtf8Array<O>,
}

impl<O: Offset> Utf8ArrayBuilder<O> {
    pub fn new() -> Self {
        Self::default()
    }
}

impl<O: Offset> EventSink for Utf8ArrayBuilder<O> {
    fn accept_str(&mut self, val: &str) -> Result<()> {
        self.array.push(Some(val));
        Ok(())
    }

    fn accept_owned_str(&mut self, val: String) -> Result<()> {
        self.array.push(Some(val));
        Ok(())
    }

    fn accept_default(&mut self) -> Result<()> {
        self.array.push::<String>(Some(String::new()));
        Ok(())
    }

    fn accept_null(&mut self) -> Result<()> {
        self.array.push::<String>(None);
        Ok(())
    }

    fn accept_some(&mut self) -> Result<()> {
        Ok(())
    }

    fn accept(&mut self, event: Event<'_>) -> Result<()> {
        match event {
            Event::Some => self.accept_some(),
            Event::Default => self.accept_default(),
            Event::Null => self.accept_null(),
            Event::Str(val) => self.accept_str(val),
            Event::OwnedStr(val) => self.accept_owned_str(val),
            ev => fail!("Cannot handle event {ev} in BooleanArrayBuilder"),
        }
    }
}

impl<O: Offset> ArrayBuilder<Box<dyn Array>> for Utf8ArrayBuilder<O> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>> {
        Ok(Box::new(<Utf8Array<_> as From<_>>::from(self.array)))
    }
}

impl<B: ArrayBuilder<Box<dyn Array>>> ArrayBuilder<Box<dyn Array>> for ListArrayBuilder<B, i64> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        let values = self.builder.into_array()?;
        let array = ListArray::try_new(
            DataType::LargeList(Box::new(Field::new(
                self.item_name,
                values.data_type().clone(),
                self.nullable,
            ))),
            Buffer::from(self.offsets),
            values,
            Some(Bitmap::from(self.validity)),
        )?;
        Ok(Box::new(array))
    }
}

impl<B: ArrayBuilder<Box<dyn Array>>> ArrayBuilder<Box<dyn Array>> for ListArrayBuilder<B, i32> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        let values = self.builder.into_array()?;
        let array = ListArray::try_new(
            DataType::List(Box::new(Field::new(
                self.item_name,
                values.data_type().clone(),
                self.nullable,
            ))),
            Buffer::from(self.offsets),
            values,
            Some(Bitmap::from(self.validity)),
        )?;
        Ok(Box::new(array))
    }
}

impl<B: ArrayBuilder<Box<dyn Array>>> ArrayBuilder<Box<dyn Array>> for MapArrayBuilder<B> {
    fn box_into_array(self: Box<Self>) -> Result<Box<dyn Array>> {
        (*self).into_array()
    }

    fn into_array(self) -> Result<Box<dyn Array>>
    where
        Self: Sized,
    {
        let keys = self.key_builder.into_array()?;
        let vals = self.val_builder.into_array()?;

        // TODO: fix nullability of different fields
        let entries_type = DataType::Struct(vec![
            Field::new("key", keys.data_type().clone(), false),
            Field::new("value", vals.data_type().clone(), false),
        ]);

        let entries = StructArray::try_new(entries_type.clone(), vec![keys, vals], None)?;
        let entries: Box<dyn Array> = Box::new(entries);

        let map_type = DataType::Map(Box::new(Field::new("entries", entries_type, false)), false);

        let array = MapArray::try_new(
            map_type,
            self.offsets.into(),
            entries,
            Some(self.validity.into()),
        )?;
        Ok(Box::new(array))
    }
}
