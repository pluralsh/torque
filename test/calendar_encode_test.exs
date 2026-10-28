defmodule Torque.CalendarEncodeTest do
  use ExUnit.Case, async: true
  use ExUnitProperties

  # The NIF writes Calendar.ISO structs itself; everything else it declines
  # goes through the Torque.Encoder implementations. Either way the result
  # must be what the Elixir to_iso8601 functions produce. The properties call
  # Torque.Native, which has no retry, so they fail if the NIF declines.
  defp expected(%mod{} = value), do: {:ok, Jason.encode!(mod.to_iso8601(value))}

  defp microsecond do
    gen all(precision <- integer(0..6), value <- integer(0..999_999)) do
      {value, precision}
    end
  end

  defp date_fields do
    fixed_map(%{
      year: one_of([integer(-9999..9999), integer(-100_000..100_000)]),
      month: integer(1..12),
      day: integer(1..31)
    })
  end

  defp time_fields do
    fixed_map(%{
      hour: integer(0..23),
      minute: integer(0..59),
      second: integer(0..59),
      microsecond: microsecond()
    })
  end

  defp zone do
    one_of([
      constant({"Etc/UTC", "UTC", 0, 0}),
      tuple({constant("Etc/UTC"), constant("UTC"), integer(-86_400..86_400), constant(0)}),
      tuple(
        {constant("America/New_York"), constant("EST"), integer(-86_400..86_400),
         member_of([0, 1800, 3600])}
      )
    ])
  end

  property "Date matches Date.to_iso8601/1" do
    check all(fields <- date_fields()) do
      date = struct(Date, fields)
      assert Torque.Native.encode(date) == expected(date)
    end
  end

  property "Time matches Time.to_iso8601/1" do
    check all(fields <- time_fields()) do
      time = struct(Time, fields)
      assert Torque.Native.encode(time) == expected(time)
    end
  end

  property "NaiveDateTime matches NaiveDateTime.to_iso8601/1" do
    check all(date <- date_fields(), time <- time_fields()) do
      naive = struct(NaiveDateTime, Map.merge(date, time))
      assert Torque.Native.encode(naive) == expected(naive)
    end
  end

  property "DateTime matches DateTime.to_iso8601/1" do
    check all(
            date <- date_fields(),
            time <- time_fields(),
            {time_zone, abbr, utc, std} <- zone()
          ) do
      fields =
        Map.merge(date, time)
        |> Map.merge(%{time_zone: time_zone, zone_abbr: abbr, utc_offset: utc, std_offset: std})

      datetime = struct(DateTime, fields)
      assert Torque.Native.encode(datetime) == expected(datetime)
    end
  end

  test "values the NIF declines still encode through the protocol" do
    big_year = %Date{year: 10 ** 20, month: 1, day: 1}
    assert {:error, :unhandled_struct} = Torque.Native.encode(big_year)
    assert Torque.encode(big_year) == expected(big_year)
    assert {:ok, ~s("100000000000000000000-01-01")} = Torque.encode(big_year)
  end

  test "invalid fields fail as the Elixir implementation does" do
    assert_raise FunctionClauseError, fn ->
      Torque.encode(%Time{hour: 24, minute: 0, second: 0, microsecond: {0, 0}})
    end
  end

  test "calendar structs nested in containers, pretty and iodata" do
    term = %{at: ~U[2026-10-27 12:34:56.120Z], days: [~D[2026-01-01], ~T[01:02:03]]}
    json = ~s({"at":"2026-10-27T12:34:56.120Z","days":["2026-01-01","01:02:03"]})

    assert {:ok, ^json} = Torque.Native.encode(term)
    assert ^json = IO.iodata_to_binary(Torque.encode_to_iodata(term))
    assert {:ok, pretty} = Torque.encode(term, pretty: true)
    assert Jason.decode!(pretty) == Jason.decode!(json)
  end

  test "a struct field that is itself unencodable is never reached" do
    assert {:error, :unhandled_struct} =
             Torque.encode(%URI{host: self()})
  end
end
