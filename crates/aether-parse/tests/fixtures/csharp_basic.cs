using System;
using System.Collections.Generic;
using static System.Math;
using Json = System.Text.Json;
using Acme.Data;

namespace Acme.Billing;

/// <summary>Charges customers.</summary>
public class PaymentService : ServiceBase, IPaymentService, IDisposable
{
    private readonly IRepository _repository;
    private ILogger Logger { get; }

    public PaymentService(IRepository repository, ILogger logger)
    {
        _repository = repository;
        Logger = logger;
    }

    public Receipt Charge(Customer customer, decimal amount)
    {
        var invoice = new Invoice(customer);
        invoice.Validate();
        _repository.Save(invoice);
        this._repository.Flush();
        Logger.Info("charged");
        base.Audit(customer);
        var total = Max(amount, 0);
        return Receipt.From(invoice, Format(total));
    }

    private string Format(decimal value) => value.ToString();

    public IEnumerable<Receipt> History(Customer customer)
    {
        foreach (Receipt receipt in _repository.Find(customer))
        {
            receipt.Print();
        }
        return _repository?.Find(customer);
    }

    public abstract class Handler
    {
        public abstract void Handle(Invoice invoice);

        public void HandleAll(List<Invoice> invoices)
        {
            invoices.ForEach(invoice => Handle(invoice));
        }
    }

    public void Notify(Action<Receipt> onCharged, Receipt receipt)
    {
        onCharged(receipt);
        Audit(receipt);
    }

    public void Dispose() { }
}

public interface IPaymentService
{
    Receipt Charge(Customer customer, decimal amount);

    bool CanCharge(Customer customer) => customer.IsActive();
}

public struct Money : IEquatable<Money>
{
    public decimal Amount;

    public bool Equals(Money other) => Amount == other.Amount;
}

public record Receipt(Invoice Invoice, string Text)
{
    public static Receipt From(Invoice invoice, string text) => new Receipt(invoice, text);
}

public record struct Point(int X, int Y);

public enum Status { Active, Retired }

public delegate void ChargedHandler(Receipt receipt);
