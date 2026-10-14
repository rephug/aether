using Xunit;
using NUnit.Framework;
using Microsoft.VisualStudio.TestTools.UnitTesting;

namespace Acme.Billing.Tests
{
    public class PaymentServiceTests
    {
        private readonly PaymentService _service = new PaymentService(null, null);

        [Fact]
        public void Charge_ReturnsReceipt_WhenCustomerIsActive()
        {
            _service.Charge(null, 1m);
        }

        [Fact(DisplayName = "refunds go back to the card")]
        public void Refund() { }

        /// <summary>
        /// Declined cards raise an error.
        /// </summary>
        [Theory]
        [InlineData(1)]
        public void Decline(int amount) { }

        [Test, Description("totals add up")]
        public void Totals() { }

        [TestCase(2)]
        public void TestRounding(int digits) { }

        [TestMethod("handles zero amounts")]
        public void Zero() { }

        /// <param name="amount">Amount in cents.</param>
        [Theory]
        [InlineData(5)]
        public void Rejects_NegativeAmounts(int amount) { }

        [Xunit.FactAttribute]
        public async Task SettlesAsync() { }

        [Fact(DisplayName = "parses \"null\"")]
        public void ParsesNull() { }

        [Fact(DisplayName = "caf\u00e9 menu")]
        public void CafeMenu() { }

        [Test(Description = "named description")]
        public void Named() { }

        [Fact(DisplayName = """raw "text" here""")]
        public void RawText() { }

        [DataTestMethod]
        public void DataDriven() { }

        [TestCaseSource(nameof(Cases))]
        public void FromSource(int value) { }

        [Fact(DisplayName = @"reads ""raw"" input")]
        public void ReadsRaw() { }

        public void Helper() { }

        public class WhenEmpty
        {
            [Fact]
            public void RejectsLookup() { }
        }

        public class WhenClosed
        {
            [Fact]
            public void RejectsLookup() { }
        }
    }

    public enum Mode { Fast }
}
